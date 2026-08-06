/**
 * A promise with its settling handles pulled out.
 *
 * The suites that use it are testing *order* — which of two in-flight `invoke`
 * answers lands first, and what the module does with the loser — so the resolve
 * and reject have to be reachable from outside the executor and callable at a
 * chosen moment. Not a `.test.ts`, so the runner does not collect it as a suite;
 * nothing in the app imports it.
 */
export function deferred<T>() {
	let resolve!: (value: T) => void
	let reject!: (reason?: unknown) => void
	const promise = new Promise<T>((res, rej) => {
		resolve = res
		reject = rej
	})
	return { promise, resolve, reject }
}
