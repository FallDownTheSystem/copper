/**
 * The updater surface: one adapter for the three Rust update commands.
 *
 * The plugin is never reached from here. `check_for_update`, `install_update`
 * and `get_app_version` are ordinary application commands, so no `updater:*`
 * permission exists in any capability file and `@tauri-apps/plugin-updater` is
 * not installed. If a permission error ever appears, something started calling
 * the plugin directly and that is the bug — granting `updater:default` would
 * hand the plugin API to the WebView and defeat the design.
 *
 * Module-scoped like every other adapter here, and for a reason specific to this
 * one: a download runs for as long as it runs, and the settings view can be left
 * and re-entered while it does. Component-scoped state would report `idle` on the
 * way back in, with a download still in flight.
 *
 * There is deliberately no `ready` state. `install_update` downloads and installs
 * in one call and the process is replaced by the installer, so a
 * downloaded-but-not-installed phase does not exist and a state for it would be
 * unreachable.
 */

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

import { errorMessage } from '@/lib/rustError'
import { createStartup } from '@/lib/startup'

/**
 * `upToDate` is distinct from `idle` because the row has to tell "not checked
 * yet" from "checked, nothing new". Collapsing them would make the status line
 * lie about one of the two.
 */
export type UpdateStatus = 'idle' | 'checking' | 'upToDate' | 'available' | 'downloading' | 'error'

/** Exactly what `check_for_update` returns for an available update. */
export type UpdateInfo = {
	version: string
	/** The running version, from the binary. Never a copy kept on this side. */
	currentVersion: string
	notes: string | null
	date: string | null
}

/** `total` is `null` when the server sent no `Content-Length`. */
export type UpdateProgress = {
	downloaded: number
	total: number | null
}

// --- module-scope state ------------------------------------------------------

const status = ref<UpdateStatus>('idle')
const currentVersion = ref<string | null>(null)
const available = ref<UpdateInfo | null>(null)
const progress = ref<UpdateProgress | null>(null)
const error = ref<string | null>(null)

/**
 * The percentage, or `null` when there is nothing honest to compute one from.
 *
 * `Content-Length` is optional and the Rust side passes the absence through
 * rather than substituting a zero, so this has to have a "no denominator" answer
 * and the UI has to have an indeterminate indicator to fall back to.
 */
const percentage = computed(() => {
	const current = progress.value
	if (!current || current.total === null || current.total <= 0) return null
	return Math.min(100, Math.round((current.downloaded / current.total) * 100))
})

/**
 * Whether the action button installs rather than checks.
 *
 * Driven by a retained update rather than by `status`, which matters on exactly
 * one path: a failed download leaves `status` at `error` while Rust still holds
 * the `Update` the user approved. Reading `status` alone would send them back
 * through a check they do not need, and would issue a second manifest request to
 * arrive at the same version.
 */
const canInstall = computed(() => available.value !== null)

/**
 * Whether the row should also offer a plain re-check.
 *
 * The escape from a retained update that will never install. `canInstall` alone
 * is a one-way door: if the endpoint's manifest changed under us — the artifact
 * was replaced, or the release was pulled and re-cut — the retained `Update`
 * points at a download that fails every time, and the row would keep offering
 * that same install for the life of the process with no way back to a check.
 *
 * Only in the `error` state. Offering it beside a perfectly good pending install
 * would just be two buttons for one decision.
 */
const canRecheck = computed(() => status.value === 'error' && available.value !== null)

/** True while a command is in flight, which is what disables the button — the
 *  Rust side would otherwise have to reject a concurrent call the UI allowed. */
const busy = computed(() => status.value === 'checking' || status.value === 'downloading')

async function pullVersion() {
	try {
		currentVersion.value = await invoke<string>('get_app_version')
	} catch (err) {
		console.error('[copper] could not read the app version', err)
	}
}

/**
 * One registration for the life of a settings-view visit. The pairing with
 * `dispose` is what stops repeated visits accumulating listeners and driving the
 * progress bar with doubled events.
 */
const { initialize, dispose } = createStartup(
	async () => [
		await listen<UpdateProgress>('update://progress', (event) => {
			// Ignored unless a download is actually running: a late event from a
			// download that has already failed would otherwise put the row back into a
			// progress state with no operation behind it.
			if (status.value !== 'downloading') return
			progress.value = event.payload
		}),
	],
	pullVersion,
)

// --- actions -----------------------------------------------------------------

/**
 * The only caller of `check_for_update` in the app.
 *
 * Nothing calls this on mount, on a timer, or from Rust's `setup()`. That is what
 * keeps adding an on-launch check — deferred, per R-Q60 — a one-line change
 * rather than an audit.
 */
async function checkForUpdate(): Promise<void> {
	if (busy.value) return

	status.value = 'checking'
	error.value = null
	// Cleared before the request, matching what Rust does to its own pending
	// value: a check that fails or finds nothing must not leave a stale version
	// installable from this row.
	available.value = null
	progress.value = null

	try {
		const info = await invoke<UpdateInfo | null>('check_for_update')
		if (!info) {
			status.value = 'upToDate'
			return
		}
		available.value = info
		currentVersion.value = info.currentVersion
		status.value = 'available'
	} catch (err) {
		error.value = errorMessage(err)
		status.value = 'error'
	}
}

/**
 * Installs the update the last check found.
 *
 * On success this never resolves: the plugin launches the installer and calls
 * `std::process::exit(0)`, so the process is gone before a reply could cross the
 * boundary. Everything after the await is the failure path, and `available` is
 * deliberately left in place there so the button stays an Install.
 */
async function installUpdate(): Promise<void> {
	if (busy.value || !available.value) return

	status.value = 'downloading'
	error.value = null
	progress.value = { downloaded: 0, total: null }

	try {
		await invoke('install_update')
	} catch (err) {
		error.value = errorMessage(err)
		status.value = 'error'
		progress.value = null
	}
}

export function useUpdater() {
	return {
		status: readonly(status),
		currentVersion: readonly(currentVersion),
		available: readonly(available),
		progress: readonly(progress),
		percentage,
		canInstall,
		canRecheck,
		busy,
		error: readonly(error),
		initialize,
		dispose,
		checkForUpdate,
		installUpdate,
	}
}
