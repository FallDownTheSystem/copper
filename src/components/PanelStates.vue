<script setup lang="ts">
/**
 * The load, error, empty and non-fatal status presentations.
 *
 * These are four separate domains, not one flag. Collapsing them is what makes a
 * background reload unmount an open editor and a failed mutation destroy the
 * draft it was supposed to preserve.
 */

const { loadState, loadError, storeStatus, storeErrorEvent, retry } = useSpace()

/** Nothing at all for the first 400ms, then a skeleton — never a spinner, and
 *  never the empty state, which would flash "No notes yet" and steal focus
 *  before the real space arrives. */
const skeletonVisible = ref(false)
let timer: ReturnType<typeof setTimeout> | undefined

watch(
	loadState,
	(state) => {
		clearTimeout(timer)
		skeletonVisible.value = false
		if (state !== 'loading') return
		timer = setTimeout(() => (skeletonVisible.value = true), 400)
	},
	{ immediate: true },
)

onBeforeUnmount(() => clearTimeout(timer))

/** An errored space blocks mutations at the store — the file cannot be read, so
 *  writing would destroy something. A space that is merely unwatched stays fully
 *  writable, so the two must never share one presentation. */
const erroredMessage = computed(() =>
	storeStatus.value.errored
		? (storeErrorEvent.value?.message ?? 'This space cannot be read right now.')
		: null,
)
</script>

<template>
	<div>
		<p
			v-if="storeStatus.startupNotice"
			class="text-text-secondary border-separator bg-surface-hover mx-3 mt-2 rounded-md border px-2 py-1.5 text-meta break-words whitespace-pre-line"
		>
			{{ storeStatus.startupNotice }}
		</p>

		<!-- `break-words`: these carry messages from Rust, and a Windows path is a
		     long run with no break opportunity in it — the panel must never scroll
		     horizontally. -->
		<p
			v-if="erroredMessage"
			class="text-text-primary border-destructive/40 bg-destructive/10 mx-3 mt-2 rounded-md border px-2 py-1.5 text-meta break-words"
		>
			<span class="font-semibold">This space is out of sync.</span>
			{{ erroredMessage }} Changes cannot be saved until it can be read again.
		</p>

		<p
			v-else-if="!storeStatus.watching && storeStatus.path"
			class="text-text-secondary mx-3 mt-2 text-meta"
		>
			Not watching this file for outside changes. Notes still save normally.
		</p>

		<!-- Loading renders nothing at all for the first 400ms, then a fixed-height
		     skeleton matching the list's shape: one section header plus three note
		     rows. Never a spinner. The branch is on `loading` rather than on
		     `skeletonVisible`, so the list cannot slip through during those 400ms
		     and flash an empty state. -->
		<div v-if="loadState === 'loading'" aria-hidden="true">
			<!-- `px-5`, not the `px-4` the rows themselves wear: these branches replace
			     the slot's `px-1` wrapper rather than rendering inside it, so landing on
			     the same 20px leading-mark column costs the 4px here that the rows get
			     from the wrapper. -->
			<div v-if="skeletonVisible" class="px-5 pt-3">
				<!-- The one place a capsule is the right shape: these bars stand for
				     lines of text, not for controls, and at 12 and 16px tall no
				     rectangular corner off the panel's scale survives the browser's
				     radius clamping anyway. -->
				<div class="bg-surface-hover h-3 w-24 rounded-full" />
				<div v-for="row in 3" :key="row" class="mt-3 flex gap-2">
					<div class="bg-surface-hover size-4 shrink-0 rounded-full" />
					<div class="bg-surface-hover h-4 flex-1 rounded-full" />
				</div>
			</div>
		</div>

		<!-- Error: a store failure must never be indistinguishable from an empty
		     space. Retry re-opens by path; `get_active_space` would return the
		     in-memory document and appear to succeed while changing nothing. -->
		<div v-else-if="loadState === 'error'" class="px-5 pt-3">
			<p class="text-text-primary text-body font-semibold">Couldn't open this space.</p>
			<p class="text-text-secondary mt-1 text-meta">Check the file still exists, then try again.</p>
			<p v-if="loadError" class="text-text-secondary mt-1 text-meta break-words">{{ loadError }}</p>
			<button type="button" class="panel-button mt-2" @click="retry">Try again</button>
		</div>

		<slot v-else />
	</div>
</template>
