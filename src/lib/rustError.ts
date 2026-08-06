/**
 * One shape of Rust error, unwrapped in one place.
 *
 * Task-003 §8.6 serialises every `StoreError` — and task-002's `ShellError`
 * after it — as a flat `{ kind, message }`, so an `invoke` rejection carries an
 * object rather than an `Error`. Every adapter has to reach through it, and a
 * second copy of that reach is the first thing to go stale when the wire shape
 * changes.
 *
 * It lives in `lib/` rather than in `useSpace`, where it started, because
 * `useAttachments` needs it too and taking it from an adapter that in turn
 * imports `useAttachments` would close a module cycle. `useSpace` re-exports it
 * so the existing callers and the auto-import entry are unchanged.
 */
export function errorMessage(error: unknown): string {
	if (error && typeof error === 'object' && 'message' in error) {
		return String((error as { message: unknown }).message)
	}
	return String(error)
}
