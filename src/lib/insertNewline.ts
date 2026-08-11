/**
 * Inserts a line break at the caret, the way the browser would have.
 *
 * Exists for the one chord Chromium gives no default editing command.
 * Shift+Enter maps to `InsertNewline` in a textarea, so its handlers give a
 * newline by *declining* the keydown — the field keeps its native undo stack
 * and its IME behaviour. Ctrl+Enter maps to nothing: a handler that declines
 * it hands the press to a browser that does nothing with it, which was the
 * "Ctrl+Enter does nothing in the composer" bug (user report, 2026-08-11).
 *
 * `execCommand` is deprecated on paper but is the only scripted insertion the
 * field treats as its own typing: the native undo stack keeps the step, and
 * the field fires its own `input` event so the component's model follows. The
 * `setRangeText` fallback loses the undo step but still beats a dead chord —
 * it is also the path the test environment takes, happy-dom shipping no
 * `execCommand` — and the synthetic `input` event is what keeps the model in
 * step there.
 */
export function insertNewline(field: HTMLTextAreaElement) {
	field.focus()
	if (typeof document.execCommand === 'function' && document.execCommand('insertText', false, '\n')) {
		return
	}
	const { selectionStart, selectionEnd } = field
	field.setRangeText('\n', selectionStart ?? field.value.length, selectionEnd ?? field.value.length, 'end')
	field.dispatchEvent(new Event('input', { bubbles: true }))
}
