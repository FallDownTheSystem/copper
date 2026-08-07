/**
 * The image type of some bytes, by magic number.
 *
 * `attachment_full` returns the blob raw — `tauri::ipc::Response` carries bytes
 * and nothing else — so the type has to be recovered here to build a `Blob` the
 * WebView will decode. Recovered rather than taken from the attachment's `mime`
 * field, which is hand-editable in the `.copper` document and is exactly the
 * thing Rust refuses to trust anywhere else.
 *
 * This is **not** a security boundary and must not be mistaken for one. Rust has
 * already sniffed the bytes on disk and refused to send anything
 * `thumb::is_thumbnailable` rejects; whatever arrives here is an image. All this
 * decides is which decoder the WebView reaches for, and a wrong answer costs a
 * broken picture. The five entries are the same five Rust will hand over, so a
 * `null` from here means the two have drifted apart.
 */

/** `at` is where the bytes sit, so WebP's second marker does not need a second
 *  entry. */
const SIGNATURES: readonly { mime: string; at: number; bytes: readonly number[] }[] = [
	{ mime: 'image/png', at: 0, bytes: [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a] },
	{ mime: 'image/jpeg', at: 0, bytes: [0xff, 0xd8, 0xff] },
	// `GIF8`, which covers both 87a and 89a.
	{ mime: 'image/gif', at: 0, bytes: [0x47, 0x49, 0x46, 0x38] },
	{ mime: 'image/bmp', at: 0, bytes: [0x42, 0x4d] },
]

function matches(bytes: Uint8Array, at: number, wanted: readonly number[]): boolean {
	if (bytes.length < at + wanted.length) return false
	return wanted.every((byte, index) => bytes[at + index] === byte)
}

export function imageMime(buffer: ArrayBuffer): string | null {
	const bytes = new Uint8Array(buffer)

	for (const signature of SIGNATURES) {
		if (matches(bytes, signature.at, signature.bytes)) return signature.mime
	}

	// WebP is a RIFF container, so the form is only known from the second marker —
	// `RIFF????WEBP`. Checking `RIFF` alone would claim a WAV file as an image.
	const riff = [0x52, 0x49, 0x46, 0x46]
	const webp = [0x57, 0x45, 0x42, 0x50]
	if (matches(bytes, 0, riff) && matches(bytes, 8, webp)) return 'image/webp'

	return null
}
