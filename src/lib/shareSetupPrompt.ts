/**
 * The hand-off prompt the Share setup guide puts on the clipboard.
 *
 * It is a module constant rather than a string in `SettingsView` for one reason:
 * it is prose about a deployment, and every fact in it is copied from
 * `worker/README.md`. Keeping it in its own file makes that pairing visible —
 * **anything edited in `worker/README.md` has to be edited here too**, and the
 * reverse. The Settings view has no other business knowing wrangler's command
 * names.
 *
 * **It tells the assistant what it must not do.** The pairing secret is the one
 * value in the whole feature that is generated inside Copper and never leaves the
 * two machines; an assistant that helpfully invented one would hand the user a
 * key it had also seen. So the prompt names that boundary explicitly rather than
 * leaving it to be inferred from an absence.
 *
 * Plain text with no Markdown fences, because it is pasted into a chat box rather
 * than rendered: a fence that arrives half-escaped is worse than an indent.
 */
export const SHARE_SETUP_PROMPT = `Help me deploy Copper's device-share relay to my own free Cloudflare account.

Copper is a Windows notes panel. Its Share feature sends a note from one of my machines to the other through a relay Worker that I host myself. Copper encrypts every message before it leaves, so the relay only ever holds ciphertext. Deploying that Worker is the only setup step that happens outside the app.

Walk me through the deployment, or run it for me if you can run commands on this machine.

SOURCE
The Worker is the worker/ directory of https://github.com/FallDownTheSystem/copper. I can clone the repository or download just that folder. Every command runs from inside worker/. There is nothing to install into it: no package.json, no node_modules, no build step.

COMMANDS, IN ORDER
1. pnpm dlx wrangler@4 login
   Signs me in. This opens a browser window.
2. pnpm dlx wrangler@4 kv namespace create MAILBOX
   Creates the KV namespace and prints an id. Note the space in "kv namespace"; the older "kv:namespace" spelling with a colon is deprecated.
3. Open wrangler.jsonc and paste that id over the placeholder in kv_namespaces.
4. pnpm dlx wrangler@4 secret put RELAY_TOKEN
   Wrangler prompts for the value and stores it as an encrypted secret, so it is never written into wrangler.jsonc. Invent a long random string rather than a memorable password, and keep a copy: I type it into both machines.
5. pnpm dlx wrangler@4 deploy
   Publishes the Worker and prints the URL it published to, of the form https://copper-relay.<my-subdomain>.workers.dev. Every free account gets a workers.dev subdomain automatically, so I do not need a domain of my own; a brand-new account may have to register that subdomain the first time it deploys.

WHICH RUNNER TO USE
Inside a clone of the Copper repository, plain npx fails with EBADDEVENGINES: the repository's root package.json pins pnpm through devEngines, and npm refuses to run under that pin from anywhere inside the repository. Use pnpm dlx wrangler@4 there, exactly as written above. If I downloaded only the worker/ folder and it sits outside any Copper checkout, plain npx wrangler@4 works too.

WHAT RELAY_TOKEN IS FOR
It is a quota guard, not a confidentiality control. It stops a stranger who finds the Worker's URL from filling my KV namespace and spending my free tier. Confidentiality comes from the pairing secret, which never leaves my two machines.

WHAT TO GIVE ME AT THE END
1. The relay URL that deploy printed.
2. The RELAY_TOKEN value that was set, so I can paste it into both machines.

Then remind me of the three parts you must not do for me:
- The pairing secret is generated inside Copper, under Settings > Share, with the Generate button. Never invent one. I generate it on one machine and paste the value into the other.
- The relay URL, the relay token and the pairing secret must be identical on both machines.
- "This device is" must differ: First on one machine and Second on the other. If both machines are set to the same one, nothing is ever delivered in either direction, and nothing detects it.
`
