/**
 * The share rows the command palette offers.
 *
 * A separate list from `settingsActions`, and deliberately so. That one is a
 * `Record<PreferenceScope, …>` written out per scope, and the union it is keyed
 * by is the **exhaustive registry of `settings.json` writes**. Share
 * configuration is not one: it lives in its own `share.json`, because every
 * field of the `Settings` struct is serialised to the WebView by `get_settings`
 * and a pairing secret must not be. Adding a `share` member to `PreferenceScope`
 * would put a non-`settings.json` write into the one union whose job is to
 * enumerate them.
 *
 * Only the enable toggle is here. A relay URL, a token and a secret are all
 * values to be typed and pasted, which is not something a palette row can do —
 * so those rows open Settings, exactly as the colour and vibrancy rows do and
 * for the same reason.
 *
 * Derived inside a function rather than as a module-scope constant, for the
 * reason `settingsActions` records: the label reads live state, so a list built
 * at import time would capture the config before the first pull resolved.
 */

import type { PaletteAction } from './settingsActions'
import { useDeviceShare } from './useDeviceShare'

export function shareActions(): PaletteAction[] {
	const { config, ready, patchConfig } = useDeviceShare()
	const { showSettings } = useView()

	// Nothing at all until the first pull resolves. A toggle labelled `Off`
	// because the answer has not arrived yet is a claim, and the one press it
	// invites would enable a feature the user may already have on.
	if (!ready.value) return []

	const configured = config.value.relayUrl !== '' && config.value.tokenSet && config.value.secretSet

	return [
		{
			id: 'share-enabled',
			label: 'Send notes to my other device',
			value: config.value.enabled ? 'On' : 'Off',
			// Turning it on with nothing configured would enable a feature that then
			// does nothing, so that press goes to the fields instead.
			run: () =>
				configured || config.value.enabled
					? patchConfig({ enabled: !config.value.enabled })
					: showSettings(),
		},
	]
}
