/**
 * The real `<TransitionGroup>`, in every suite.
 *
 * @vue/test-utils stubs `Transition` and `TransitionGroup` by default, and the
 * stub renders its children under a `<transition-group-stub>` element. That was
 * invisible while the list's rowgroups were plain divs, but the note list and
 * the attachment tray now *are* TransitionGroups — the stub breaks the
 * accessibility tree the axe suite asserts (an `<li>` under a stub has no list
 * parent) and swaps out the exact machinery the list-motion tests exercise.
 *
 * `Transition` stays stubbed: nothing under test depends on it, and the reka-ui
 * overlays that use it are quicker as stubs.
 *
 * Deterministic all the same: the enter/leave hooks in `useListTransition`
 * report done synchronously when `Element.animate` is missing, and the suites
 * that stub `animate` finish it on a microtask — either way a removed row is
 * out of the DOM within a `settle`.
 */

import { config } from '@vue/test-utils'

// Merged, never assigned: the defaults live *in* `config.global.stubs`
// (`{ transition: true, 'transition-group': true }`), so replacing the object
// silently un-stubs `Transition` as well — which turned the status toast's
// `mode="out-in"` swap into a leave that waits on animation frames no fake
// timer runs.
config.global.stubs = { ...config.global.stubs, 'transition-group': false }
