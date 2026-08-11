<script setup lang="ts">
/**
 * The visible half of `useStatusMessage`: vue-sonner's stack, dressed as
 * Copper's pill.
 *
 * Sonner replaced the single-pill `StatusLine` on the user's 2026-08-11
 * direction — a second message used to take the pill from the first, and
 * marking five notes done left one toast saying what the fifth press did.
 * Now each message stacks, and each `Undo` undoes its own press.
 *
 * **`unstyled`, and the classes are the old pill's own.** Sonner's look is
 * gated behind `data-styled='true'`; switching it off leaves only the stack
 * mechanics (positioning, lift, swipe), and the classes below re-create
 * `StatusLine`'s pill — same tokens, same shadow, same button. The error
 * colours live in the style block rather than in a `classes.error` string,
 * because Sonner *adds* the type class beside the base one and two `bg-*`
 * utilities on one element resolve by stylesheet order, not by which was meant.
 *
 * **Theme is bound, not `"system"`.** Copper resolves its own theme choice;
 * asking Sonner to consult the OS would let the two disagree.
 */
import { Toaster } from 'vue-sonner'
import 'vue-sonner/style.css'

const { isDark } = useTheme()

/** Matches the wrapper band the old pill sat in: `px-3 pb-2` — the field
 *  boxes' edge, and the 8px list rhythm off the composer. Given to Sonner as
 *  offsets because its container is a positioned overlay, not a flex child.
 *  The panel is narrower than Sonner's 600px "mobile" breakpoint, so the
 *  mobile offsets are the ones that actually apply; the desktop pair is kept
 *  equal so nothing changes if the panel ever widens. */
const OFFSET = { bottom: 8, left: 12, right: 12 }
</script>

<template>
	<!-- `expand`: the stack renders as a fully visible list rather than Sonner's
	     default collapsed fan, where the newest pill covers the older ones almost
	     entirely and two quick actions read as one toast replacing another — the
	     exact impression the stack exists to end (user report, 2026-08-11).
	     `visible-toasts` still caps how many stand open at once. -->
	<Toaster
		:theme="isDark ? 'dark' : 'light'"
		position="bottom-center"
		expand
		:offset="OFFSET"
		:mobile-offset="OFFSET"
		:gap="6"
		:visible-toasts="4"
		:toast-options="{
			unstyled: true,
			classes: {
				toast:
					'toast-pill pointer-events-auto bg-toast-surface border-separator text-text-primary flex w-full items-center gap-2 rounded-md border px-2 py-1.5 text-meta',
				title: 'min-w-0 break-words',
				actionButton:
					'focus-ring text-accent-text hover:bg-surface-hover active:bg-surface-active -my-0.5 ml-auto shrink-0 rounded-md px-1.5 py-0.5 font-semibold whitespace-nowrap transition-colors duration-fast',
			},
		}"
	/>
</template>

<!-- Unscoped on purpose: Sonner renders its own subtree and the scope
     attribute reaches only its root. Every selector is anchored on Sonner's
     own data attributes, so nothing here can leak. -->
<style>
/* Anchored to the shell's overlay cell rather than the window. Sonner fixes
   its container to the viewport, which in this panel would sit it on top of
   the composer; the host cell is the note-list region, so `absolute` pins the
   stack to the list's foot — where the pill has always lived. The doubled
   attribute outweighs the library's own single-attribute rule and its mobile
   media block without depending on import order. */
.status-toaster-host [data-sonner-toaster][data-sonner-toaster] {
	position: absolute;
}

/* The old pill's error dress, by toast type. See the component comment for why
   this is not a `classes.error` string. */
[data-sonner-toast][data-type='error'] {
	background: var(--surface-danger);
	border-color: color-mix(in oklab, var(--destructive) 40%, transparent);
}
</style>
