/**
 * Copper's device-share relay: a mailbox that only ever holds ciphertext.
 *
 * Four routes and nothing else. The Worker never sees a note body, an
 * attachment name or a device name — it is handed a sealed blob, stores it
 * under a TTL, hands it back once, and forgets it. Losing this Worker, or the
 * KV namespace behind it, discloses nothing.
 *
 * Two properties shape every line below.
 *
 * **The free plan gives an invocation 10 ms of CPU.** Buffering a 20 MiB body
 * into an ArrayBuffer, or base64-encoding it, spends far more than that. So
 * `POST /send` hands `request.body` — the ReadableStream — straight to
 * `KV.put`, and `GET /msg` hands the stored stream straight back. Nothing on
 * the storage path is parsed, copied or re-encoded in either direction.
 *
 * **KV's write and delete budgets are 1,000/day each, and `list` has its own.**
 * This Worker never calls `list`. The protocol carries its own cursors — a
 * head pointer written by the sender and an acknowledged pointer written by
 * the reader — so an idle poll costs one `get` from the 100,000/day read
 * budget and nothing else.
 *
 * Keys, all in one namespace:
 *
 *   m:<mailbox>:<seq padded to 20 digits>   the ciphertext          TTL  7 days
 *   h:<mailbox>                             highest seq written     TTL 30 days
 *   a:<mailbox>                             highest seq acknowledged TTL 30 days
 *
 * The cursors outlive every ciphertext they can point at, so a live cursor
 * never points into an expired range.
 */

const MESSAGE_TTL_SECONDS = 604800 // 7 days
const CURSOR_TTL_SECONDS = 2592000 // 30 days

/** Matches the client's SHARE_MAX_PAYLOAD_BYTES. KV's own value cap is 25 MiB. */
const MAX_BODY_BYTES = 20 * 1024 * 1024

/** A 24-byte nonce plus a 16-byte Poly1305 tag. Nothing shorter can be a message. */
const MIN_BODY_BYTES = 40

/** 2^64 - 1. Sequences are u64 on the Rust side and must round-trip exactly. */
const MAX_SEQ = 18446744073709551615n

const BOX_PATTERN = /^[0-9a-f]{32}$/
const SEQ_PATTERN = /^[0-9]{1,20}$/

export default {
	async fetch(request, env) {
		const url = new URL(request.url)
		const route = `${request.method} ${url.pathname}`

		// **Recognised first, authenticated second.** These four are the whole
		// surface; anything else is 404 whether or not a token came with it, so the
		// contract "no other path returns anything but 404" holds for every caller.
		// Nothing is read or written before the token check below, so an
		// unauthenticated request still touches KV not at all.
		switch (route) {
			case 'POST /send':
			case 'GET /head':
			case 'GET /msg':
			case 'DELETE /msg':
				break
			default:
				return status(404)
		}

		// The token is a quota guard, not a confidentiality control — the payloads
		// are already encrypted end to end. What it buys is that a stranger who
		// finds the URL cannot fill the user's KV namespace.
		if (!(await authorised(request, env))) {
			return status(401)
		}

		switch (route) {
			case 'POST /send':
				return send(request, url, env)
			case 'GET /head':
				return head(url, env)
			case 'GET /msg':
				return message(url, env)
			default:
				return remove(url, env)
		}
	},
}

/**
 * Whether the request carries the configured bearer token.
 *
 * The **comparison** is constant-time; the surrounding request handling is not,
 * and nothing here pretends otherwise. `crypto.subtle.timingSafeEqual` throws
 * on operands of different lengths, so a length mismatch cannot be allowed to
 * return early — it would leak the token's length through response timing.
 * Instead it compares a buffer against itself, spending the same work, and
 * returns false regardless.
 */
async function authorised(request, env) {
	const encoder = new TextEncoder()
	const header = request.headers.get('Authorization') ?? ''
	const offered = encoder.encode(header.startsWith('Bearer ') ? header.slice(7) : '')
	const expected = encoder.encode(env.RELAY_TOKEN ?? '')

	// An unset secret is not an open door. `expected` is empty, so only an empty
	// offered token could match it — and that path is refused here explicitly
	// rather than left to the comparison below.
	if (expected.byteLength === 0) {
		crypto.subtle.timingSafeEqual(offered, offered)
		return false
	}
	if (offered.byteLength !== expected.byteLength) {
		crypto.subtle.timingSafeEqual(offered, offered)
		return false
	}
	return crypto.subtle.timingSafeEqual(offered, expected)
}

/**
 * The mailbox identifier, validated.
 *
 * Separate from `messageParams` below rather than one shared helper, because
 * `GET /head` has no `seq` at all and a shared validator would reject every
 * poll. Returns `null` when the parameter is missing or malformed; the caller
 * answers 400 before any KV call.
 */
function boxParam(url) {
	const box = url.searchParams.get('box')
	return box !== null && BOX_PATTERN.test(box) ? box : null
}

/**
 * The mailbox and the sequence number, both validated.
 *
 * `seq` crosses the wire as a decimal **string**: a 20-digit sequence exceeds
 * JavaScript's safe integer range, so it is range-checked with `BigInt` and
 * never converted to a Number.
 */
function messageParams(url) {
	const box = boxParam(url)
	if (box === null) {
		return null
	}
	const seq = url.searchParams.get('seq')
	if (seq === null || !SEQ_PATTERN.test(seq) || BigInt(seq) > MAX_SEQ) {
		return null
	}
	return { box, seq }
}

/** Keys are built only from validated values, never from raw query parameters. */
function messageKey(box, seq) {
	return `m:${box}:${seq.padStart(20, '0')}`
}

function headKey(box) {
	return `h:${box}`
}

function ackKey(box) {
	return `a:${box}`
}

function status(code) {
	return new Response(null, { status: code })
}

/**
 * Stores a sealed message, then advances the head pointer.
 *
 * The two writes are separate KV operations and the second can fail on its own.
 * That case answers **202** rather than 204: the message is stored but not yet
 * announced, and the client needs to be able to tell "delivered" from "sitting
 * there unseen". It is not lost — the head is a high-water mark and the reader
 * walks every sequence up to it, so the next send that lands its head write
 * announces this message too.
 */
async function send(request, url, env) {
	const params = messageParams(url)
	if (params === null) {
		return status(400)
	}

	// Read the length rather than the body. `request.body` is passed through
	// untouched below, so this is the only chance to refuse an oversized upload
	// before it is streamed into KV — and the only way to refuse it without
	// spending the CPU that reading it would cost.
	const declared = request.headers.get('Content-Length')
	if (declared === null || !/^[0-9]+$/.test(declared)) {
		return status(411)
	}
	const length = Number(declared)
	if (length < MIN_BODY_BYTES || length > MAX_BODY_BYTES) {
		return status(413)
	}
	if (request.body === null) {
		return status(411)
	}

	await env.MAILBOX.put(messageKey(params.box, params.seq), request.body, {
		expirationTtl: MESSAGE_TTL_SECONDS,
	})

	try {
		await env.MAILBOX.put(headKey(params.box), params.seq, {
			expirationTtl: CURSOR_TTL_SECONDS,
		})
	} catch {
		return status(202)
	}

	return status(204)
}

/**
 * The highest sequence written to this mailbox, or null.
 *
 * One KV read, and the entire cost of an idle poll. The counter is emitted as a
 * **string** for the same reason it is parsed as one.
 *
 * `?cursor=ack` reads the *acknowledged* cursor from the same route instead. It
 * is a parameter rather than a fifth path deliberately: a reader that has lost
 * `share.json` re-syncs from `a:<mailbox>`, and serving that on its own route
 * would be a route the idle poll never touches, while returning both counters
 * from one call would put a second KV read on every poll. This way an idle poll
 * still costs exactly one read, and the rare re-sync costs one more.
 */
async function head(url, env) {
	const box = boxParam(url)
	if (box === null) {
		return status(400)
	}
	const cursor = url.searchParams.get('cursor')
	if (cursor !== null && cursor !== 'head' && cursor !== 'ack') {
		return status(400)
	}

	const key = cursor === 'ack' ? ackKey(box) : headKey(box)
	const value = await env.MAILBOX.get(key, { type: 'text' })
	// The field is `head` for both cursors. One reply shape means one parser on
	// the client, and which counter it holds is decided by what was asked for.
	return Response.json({ head: value })
}

/** The stored bytes, exactly as they arrived. No base64 in either direction. */
async function message(url, env) {
	const params = messageParams(url)
	if (params === null) {
		return status(400)
	}
	const value = await env.MAILBOX.get(messageKey(params.box, params.seq), { type: 'stream' })
	if (value === null) {
		return status(404)
	}
	return new Response(value, {
		headers: { 'content-type': 'application/octet-stream' },
	})
}

/**
 * Deletes a consumed message and records how far the reader has got.
 *
 * Both in one invocation, because the acknowledged cursor is what lets a reader
 * that lost its local state re-sync exactly (`nextRead = acked + 1`) instead of
 * enumerating keys. Deleting an absent key succeeds in KV, so the client can
 * treat this as idempotent and retry it freely.
 */
async function remove(url, env) {
	const params = messageParams(url)
	if (params === null) {
		return status(400)
	}
	await env.MAILBOX.delete(messageKey(params.box, params.seq))
	await env.MAILBOX.put(ackKey(params.box), params.seq, {
		expirationTtl: CURSOR_TTL_SECONDS,
	})
	return status(204)
}
