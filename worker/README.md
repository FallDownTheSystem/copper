# Copper's device-share relay

A Cloudflare Worker and one Workers KV namespace, deployed to **your own** free
Cloudflare account. It is the mailbox Copper uses to send a note from one of
your machines to the other.

**The relay never sees your notes.** Copper encrypts every message on the
sending machine and decrypts it on the receiving one. This Worker, Cloudflare,
and anyone who can read the KV namespace see a random nonce and a block of
ciphertext. They do not see note bodies, attachment names, attachment bytes,
section names or device names. The key is derived on your two machines from a
pairing secret that is never sent anywhere.

It is delivery, not storage. Every stored message expires by itself after seven
days, whether or not anybody collects it.

## What you need

- A free Cloudflare account.
- Node.js and pnpm. There is nothing to install into this directory — no
  `package.json`, no `node_modules`, no build step.

## Deploy

Run all five commands from inside this `worker/` directory.

The commands use `pnpm dlx` rather than `npx` because the repository's root
`package.json` pins pnpm through `devEngines`, and npm refuses to run under
that pin from anywhere inside the repository (`EBADDEVENGINES`). Plain
`pnpm dlx wrangler@4` works fine from a directory outside this checkout.

1. Sign in. This opens a browser window.

   ```
   pnpm dlx wrangler@4 login
   ```

2. Create the KV namespace. This prints an `id`.

   ```
   pnpm dlx wrangler@4 kv namespace create MAILBOX
   ```

   Note the space in `kv namespace`. The older `kv:namespace` spelling with a
   colon is deprecated.

3. Open `wrangler.jsonc` and paste that `id` over the placeholder in
   `kv_namespaces`.

4. Set the relay token. Wrangler prompts for the value and stores it as an
   encrypted secret — it is never written into `wrangler.jsonc`.

   ```
   pnpm dlx wrangler@4 secret put RELAY_TOKEN
   ```

   Invent a long random string. It is not a password you have to remember, so
   make it unguessable rather than memorable. Keep a copy: you type it into both
   machines.

5. Deploy.

   ```
   pnpm dlx wrangler@4 deploy
   ```

   Deploy prints the URL it published to, of the form
   `https://copper-relay.<your-subdomain>.workers.dev`. Every free account gets
   a `workers.dev` subdomain automatically; you do not need a domain of your own.

## Put the values into Copper

Open **Settings → Share** on **both** machines.

| Field          | Value                                                                 |
| -------------- | --------------------------------------------------------------------- |
| Relay URL      | the `https://copper-relay.<subdomain>.workers.dev` URL deploy printed |
| Relay token    | the `RELAY_TOKEN` value you set in step 4                             |
| Pairing secret | press **Generate** on one machine, then copy the value to the other   |
| This device is | **First** on one machine and **Second** on the other                  |

The relay URL, the relay token and the pairing secret must be **identical** on
both machines. **This device is** must be **different** — one First, one Second.
If both machines are set to the same one, each writes to a mailbox neither
reads, and nothing is ever delivered. Nothing detects this, so check it.

The pairing secret is shown once, at the moment it is generated. Copper never
displays a stored secret again. If you lose it, generate a new one and set it on
both machines.

## What the relay token is for

It is a quota guard, not a confidentiality control. It stops a stranger who
finds your Worker's URL from filling your KV namespace and spending your free
tier. Confidentiality comes from the pairing secret, which never leaves your
machines.

Copper clears the stored relay token whenever you change the relay URL, so the
old host's credential is never sent to a new one.

## What it costs

Nothing, on the free plan, with both machines running all day. Two devices
polling once a minute around the clock is about 2,880 requests and 2,880 KV
reads per day, against limits of 100,000 each. The scarce budgets are KV writes
and deletes at 1,000 per day each — a send costs two writes and a receive costs
one write and one delete, so the ceiling is a few hundred notes a day. The
Worker never calls KV `list`, which has its own 1,000-per-day budget.

A single note is capped at 20 MB **after encryption**. Attachments are base64
encoded inside the message, which costs about a third more than the file size,
so roughly 14 MB of attachments fit.

## The routes

Four, all requiring `Authorization: Bearer <RELAY_TOKEN>`. Anything else is 404.

| Route                       | Does                                                                       |
| --------------------------- | -------------------------------------------------------------------------- |
| `POST /send?box=&seq=`      | stores the raw request body as the message, then advances the head pointer |
| `GET /head?box=`            | `{"head": "<n>"}` or `{"head": null}`                                      |
| `GET /head?box=&cursor=ack` | the same shape, reading the acknowledged cursor instead                    |
| `GET /msg?box=&seq=`        | the stored bytes, or 404                                                   |
| `DELETE /msg?box=&seq=`     | deletes the message and records the acknowledged cursor                    |

`cursor=ack` is a parameter rather than a fifth route so that the once-a-minute
poll costs exactly one KV read. Copper asks for it only when it has lost its
local place and needs to re-sync.

Counters cross the wire as decimal strings, not JSON numbers: a 20-digit
sequence is past JavaScript's safe integer range.

## Updating or removing it

Re-deploy with `pnpm dlx wrangler@4 deploy` after any edit to `src/index.js`.

To remove the relay entirely, run `pnpm dlx wrangler@4 delete` and then delete the
KV namespace from the Cloudflare dashboard. Turn Share off in Copper on both
machines first, or they will report an unreachable relay every minute.
