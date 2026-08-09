//! The two global bindings — summon and capture — with their validation,
//! registration, rebind protocol and recording lease.
//!
//! # Two rules that are not style
//!
//! **Never `register()`, always `on_shortcut()`.** `register` takes only a
//! shortcut and stores `handler: None`; task-002 builds the plugin as a plain
//! `Builder::new().build()` with no plugin-wide handler, so the plugin's dispatch
//! closure finds nothing in either branch and silently drops the event. That
//! failure presents as a *working* registration — the chord is claimed
//! system-wide, so no other app can take it — right up until the user presses it.
//!
//! **The registry lock is never taken on the main thread.** Registering and
//! unregistering go through the plugin's `run_main_thread!`, which posts to the
//! event loop and blocks on the reply from any other thread. A command holding
//! this lock is therefore waiting on the main thread; if the main thread were
//! also waiting on this lock — a tray click cancelling a lease, say — neither
//! would move again. Every main-thread trigger hands off to a short-lived thread
//! instead, and [`shutdown`] uses `try_lock` rather than blocking exit.
//!
//! # One shape, two roles
//!
//! Each binding is a double-tap or a conventional chord, and the two are served
//! by different machinery: a double-tap by `capture`'s keyboard hook, a chord by
//! `tauri-plugin-global-shortcut`. Summon could once only be the latter; from
//! task-020 it can be either, which is why almost everything below is written
//! once against a [`Role`] rather than twice against two names.
//!
//! **A chord is never sided.** `LCtrl LCtrl` is a binding this module accepts and
//! `LCtrl+K` is not, and that asymmetry is the plugin's rather than a choice: its
//! `Modifiers` and the `RegisterHotKey` beneath it cannot express which physical
//! key a modifier came from. Only the hook sees sides, so only the bindings the
//! hook services can carry one.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{
	Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutEvent, ShortcutState as KeyState,
};

use crate::capture::{self, KeySide, ModifierFamily, WatchedTrigger};
use copper_core::store::settings::{SettingsPatch, Shortcuts};
use crate::{diagnostics, panel, store, tray, ShellError};

/// The shipped defaults. Asserted equal to task-003's `Settings::default()` in a
/// unit test — two hardcoded copies of one default is exactly the drift that
/// catches.
pub const DEFAULT_SUMMON_SHORTCUT: &str = "Ctrl+Shift+Space";
pub const DEFAULT_CAPTURE_TRIGGER: &str = "Shift Shift";

/// Registered only while the `WH_KEYBOARD_LL` hook is *not* installed, and only
/// for a role whose binding is a double-tap the hook can no longer recognise.
///
/// The insurance the spec asks for: a hook that failed to attach takes every
/// double-tap with it, and without these the user has no way to reach that action
/// at all until they open the settings view and rebind — which for summon means
/// opening a panel whose shortcut is the thing that stopped working. Deliberately
/// obscure, since they exist to be available rather than to be convenient.
const FALLBACK_CAPTURE_CHORD: &str = "Ctrl+Alt+Shift+C";
const FALLBACK_SUMMON_CHORD: &str = "Ctrl+Alt+Shift+Space";

/// How long a recording lease may stay open before Rust takes it back.
///
/// The lease unregisters the live chords and mutes the hook, so an abandoned one
/// costs the user both shortcuts until the app restarts. Generous enough that a
/// person deciding what to bind is never interrupted.
const LEASE_WATCHDOG: Duration = Duration::from_secs(120);

// --- the two roles -----------------------------------------------------------

/// Which binding is being talked about. The two are never interchangeable — one
/// reveals a window, the other reads the foreground selection — and every
/// registration carries the role whose handler it fires.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
	Summon,
	Capture,
}

/// Summon first, because that is the order [`install`] must register in: the
/// startup-failure comment on the cross-role guard depends on it.
const ROLES: [Role; 2] = [Role::Summon, Role::Capture];

impl Role {
	fn other(self) -> Self {
		match self {
			Self::Summon => Self::Capture,
			Self::Capture => Self::Summon,
		}
	}

	/// The word the user reads in an error. Lower case because it is always used
	/// mid-sentence.
	fn noun(self) -> &'static str {
		match self {
			Self::Summon => "summon",
			Self::Capture => "capture",
		}
	}

	fn hook_role(self) -> capture::TriggerRole {
		match self {
			Self::Summon => capture::TriggerRole::Summon,
			Self::Capture => capture::TriggerRole::Capture,
		}
	}
}

// --- the canonical bindings, readable without a lock -------------------------

/// `HotKey::id` is `(mods.bits() << 16) | key`, and `Modifiers` uses ten bits, so
/// `u32::MAX` is not a reachable id and serves as "nothing is bound".
const NO_CHORD: u32 = u32::MAX;

/// Read by the shortcut handlers, written by the rebind protocol.
///
/// Atomics rather than fields behind the registry lock for two reasons: a handler
/// must never contend with a rebind in progress, and a handler that took this
/// lock could deadlock against a command holding it while waiting on the main
/// thread. The value is one independent selector publishing no other memory, so
/// `Relaxed` is sufficient.
///
/// One per role and not one per *binding*: a role's chord and its insurance chord
/// are mutually exclusive, so a single id per role can never need to hold two.
static CANONICAL_SUMMON: AtomicU32 = AtomicU32::new(NO_CHORD);
static CANONICAL_CAPTURE: AtomicU32 = AtomicU32::new(NO_CHORD);

fn canonical(role: Role) -> &'static AtomicU32 {
	match role {
		Role::Summon => &CANONICAL_SUMMON,
		Role::Capture => &CANONICAL_CAPTURE,
	}
}

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// Whether a recording session is open, readable without the lock.
///
/// Every panel hide asks, and a hide happens on the main thread — so the answer
/// has to be available without taking a lock a command may be holding while it
/// waits on that same main thread. It also saves a thread spawn on the
/// overwhelmingly common case of hiding a panel nobody was recording in.
static LEASE_OPEN: AtomicBool = AtomicBool::new(false);

// --- what a binding is -------------------------------------------------------

/// A chord and whether the OS actually took it.
#[derive(Clone, Debug)]
struct Binding {
	text: String,
	chord: Shortcut,
	registered: bool,
	/// Why the last registration attempt failed, kept so the settings view can
	/// show it on a pull rather than needing an event it could never have heard.
	error: Option<String>,
}

/// What one role is bound to.
///
/// R-Q52 made these alternatives rather than a single shape: a double-tap is
/// recognised by task-005's keyboard hook, a conventional chord by the
/// global-shortcut plugin, and exactly one of the two is live at a time.
#[derive(Clone, Debug)]
enum TriggerBinding {
	DoubleTap {
		text: String,
		/// A second copy of what `capture::watch` already holds, and it earns its
		/// keep: the anti-lockout guard has to ask whether *these two bindings*
		/// overlap, and only one of them is ever in the hook's atomic. Re-parsing
		/// the text to answer would put a fallible parse on a guard that must not
		/// be able to fail. Written in exactly one place — [`binding_of`], which is
		/// also where the text is written — so the two cannot drift.
		trigger: WatchedTrigger,
	},
	Chord(Binding),
}

impl TriggerBinding {
	fn text(&self) -> &str {
		match self {
			Self::DoubleTap { text, .. } => text,
			Self::Chord(binding) => &binding.text,
		}
	}
}

/// The insurance chord for one role, and why it is not available when it is not.
///
/// The error is surfaced as that role's own error, because from the user's side
/// that is exactly what it is: a hook that will not install *and* no chord
/// standing in for it means the action cannot be reached at all.
#[derive(Clone, Debug, Default)]
struct Fallback {
	chord: Option<Shortcut>,
	error: Option<String>,
}

/// What [`begin_recording`] suspended, so the same set can be put back.
struct Lease {
	token: u64,
	summon: bool,
	capture: bool,
	summon_fallback: bool,
	capture_fallback: bool,
}

impl Lease {
	fn chord(&self, role: Role) -> bool {
		match role {
			Role::Summon => self.summon,
			Role::Capture => self.capture,
		}
	}

	fn chord_mut(&mut self, role: Role) -> &mut bool {
		match role {
			Role::Summon => &mut self.summon,
			Role::Capture => &mut self.capture,
		}
	}

	fn fallback(&self, role: Role) -> bool {
		match role {
			Role::Summon => self.summon_fallback,
			Role::Capture => self.capture_fallback,
		}
	}

	fn fallback_mut(&mut self, role: Role) -> &mut bool {
		match role {
			Role::Summon => &mut self.summon_fallback,
			Role::Capture => &mut self.capture_fallback,
		}
	}
}

struct Registry {
	summon: TriggerBinding,
	capture: TriggerBinding,
	summon_fallback: Fallback,
	capture_fallback: Fallback,
	/// Registrations `unregister` refused to retire.
	///
	/// Not merely loggable: a chord that would not retire still fires its handler,
	/// which contradicts the acceptance criterion that the old binding stops
	/// working. Retried on the next rebind and at shutdown; the handlers' canonical
	/// check keeps a lingering one inert meanwhile.
	stale: Vec<Shortcut>,
	lease: Option<Lease>,
}

impl Registry {
	/// The shipped state, built rather than parsed.
	///
	/// Constructing the chord means there is no parse to fail and therefore no
	/// `expect` on a path a release build would abort on. A unit test asserts it
	/// equals `Shortcut::from_str(DEFAULT_SUMMON_SHORTCUT)`, which is where drift
	/// between the two spellings surfaces.
	fn shipped() -> Self {
		Self {
			summon: binding_of(shipped_trigger(Role::Summon)),
			capture: binding_of(shipped_trigger(Role::Capture)),
			summon_fallback: Fallback::default(),
			capture_fallback: Fallback::default(),
			stale: Vec::new(),
			lease: None,
		}
	}

	fn binding(&self, role: Role) -> &TriggerBinding {
		match role {
			Role::Summon => &self.summon,
			Role::Capture => &self.capture,
		}
	}

	fn binding_mut(&mut self, role: Role) -> &mut TriggerBinding {
		match role {
			Role::Summon => &mut self.summon,
			Role::Capture => &mut self.capture,
		}
	}

	fn fallback(&self, role: Role) -> &Fallback {
		match role {
			Role::Summon => &self.summon_fallback,
			Role::Capture => &self.capture_fallback,
		}
	}

	fn fallback_mut(&mut self, role: Role) -> &mut Fallback {
		match role {
			Role::Summon => &mut self.summon_fallback,
			Role::Capture => &mut self.capture_fallback,
		}
	}
}

/// The shipped binding for a role, as a validated value rather than as text.
fn shipped_trigger(role: Role) -> BoundTrigger {
	match role {
		Role::Summon => BoundTrigger::Chord(Shortcut::new(
			Some(Modifiers::CONTROL | Modifiers::SHIFT),
			Code::Space,
		)),
		Role::Capture => BoundTrigger::DoubleTap(WatchedTrigger::unsided(ModifierFamily::Shift)),
	}
}

fn shipped_text(role: Role) -> &'static str {
	match role {
		Role::Summon => DEFAULT_SUMMON_SHORTCUT,
		Role::Capture => DEFAULT_CAPTURE_TRIGGER,
	}
}

fn fallback_chord_text(role: Role) -> &'static str {
	match role {
		Role::Summon => FALLBACK_SUMMON_CHORD,
		Role::Capture => FALLBACK_CAPTURE_CHORD,
	}
}

/// What a role's row says when the hook is down and its insurance chord could not
/// be registered either.
fn fallback_unavailable(role: Role) -> String {
	format!(
		"Copper couldn't install its keyboard hook, and the shortcut that stands in for it was \
		 refused too. Bind {} to a key combination instead.",
		role.noun()
	)
}

/// The one serialising lock over every shortcut mutation.
///
/// One lock rather than two, because both bindings live inside a single
/// `Shortcuts` struct that is written as a whole: two concurrent patches would
/// each persist a stale copy of the other's field, which is a lost update rather
/// than a race nobody notices.
fn registry() -> MutexGuard<'static, Registry> {
	registry_mutex()
		.lock()
		.unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn registry_mutex() -> &'static Mutex<Registry> {
	static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
	REGISTRY.get_or_init(|| Mutex::new(Registry::shipped()))
}

/// For the one caller that must not block — see [`shutdown`].
///
/// A poisoned lock is taken anyway, on the same reasoning `store::lock` uses:
/// everything behind it is small owned state, and refusing to work after one
/// panic would turn a single failure into a permanently unrebindable shortcut.
fn try_registry() -> Option<MutexGuard<'static, Registry>> {
	match registry_mutex().try_lock() {
		Ok(guard) => Some(guard),
		Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
		Err(std::sync::TryLockError::WouldBlock) => None,
	}
}

// --- validation --------------------------------------------------------------

/// A validated binding, either role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundTrigger {
	DoubleTap(WatchedTrigger),
	Chord(Shortcut),
}

/// The parser's own modifier tokens, so "is this modifiers only?" is decided by
/// the same vocabulary the parser uses rather than by a second list that can
/// drift from it.
///
/// The sided spellings are here too even though no chord may carry one: a bare
/// `LShift` is the same mistake as a bare `Shift` and deserves the same answer,
/// which is "hold it and press one more key" rather than "Copper couldn't read
/// that".
fn is_modifier_token(token: &str) -> bool {
	const MODIFIERS: [&str; 12] = [
		"Alt",
		"Option",
		"Control",
		"Ctrl",
		"Command",
		"Cmd",
		"Super",
		"Shift",
		"CommandOrControl",
		"CommandOrCtrl",
		"CmdOrCtrl",
		"CmdOrControl",
	];
	let token = token.trim();
	MODIFIERS.iter().any(|name| token.eq_ignore_ascii_case(name))
		|| double_tap_binding(token).is_some()
}

/// Windows will never deliver these, so binding one produces a shortcut that
/// silently does nothing.
fn reserved_reason(chord: &Shortcut, text: &str) -> Option<String> {
	let combining = Modifiers::SHIFT | Modifiers::CONTROL | Modifiers::ALT;
	if chord.mods.contains(Modifiers::SUPER) && !chord.mods.intersects(combining) {
		return Some(format!(
			"Windows keeps the Windows key for itself, so {text} would never reach Copper. Add \
			 Ctrl, Alt or Shift."
		));
	}
	if chord.mods.contains(Modifiers::ALT) && chord.key == Code::Tab {
		return Some(format!("{text} belongs to Windows for switching apps."));
	}
	if chord.key == Code::Delete
		&& chord.mods.contains(Modifiers::CONTROL)
		&& chord.mods.contains(Modifiers::ALT)
	{
		return Some(format!("{text} belongs to Windows and no app can take it."));
	}
	if chord.key == Code::PrintScreen {
		return Some(format!("{text} belongs to Windows for screenshots."));
	}
	None
}

/// Parses and vets a conventional chord.
///
/// Rejects before touching the OS, and with a worded reason per cause — the
/// parser's own "invalid hotkey format" covers modifier-only input and genuine
/// nonsense with one message, and those are different mistakes to make.
pub fn validate_chord(text: &str) -> Result<Shortcut, ShellError> {
	let text = text.trim();
	if text.is_empty() {
		return Err(ShellError::InvalidChord(
			"Press the keys you want to use.".to_owned(),
		));
	}

	if text.split('+').all(is_modifier_token) {
		return Err(ShellError::ModifierOnly(format!(
			"{text} is only modifier keys. Hold them and press one more key."
		)));
	}

	let chord = Shortcut::from_str(text).map_err(|_| {
		ShellError::InvalidChord(format!(
			"Copper couldn't read {text} as a shortcut. Hold the modifiers you want and press one \
			 other key."
		))
	})?;

	// Before the bare-key rule below, not after: `PrintScreen` is reserved whether
	// or not it carries a modifier, and "Windows keeps this for screenshots" tells
	// the user something the generic "add a modifier" does not.
	if let Some(why) = reserved_reason(&chord, text) {
		return Err(ShellError::Reserved(why));
	}

	if chord.mods.is_empty() && !bindable_bare(chord.key) {
		return Err(ShellError::InvalidChord(format!(
			"{text} on its own would be taken from every other app on the machine. Hold Ctrl, Alt \
			 or Shift with it."
		)));
	}

	Ok(chord)
}

/// The keys a global binding may claim with no modifier at all.
///
/// **F13–F23 only, and F1–F12 deliberately not.** The high function keys exist
/// for precisely this: no keyboard emits them by accident, and nothing else is
/// listening for them. F1–F12 are live in almost every application — binding a
/// bare F5 globally takes Refresh away from every browser on the machine — so
/// they are treated like any other single key and need a modifier.
///
/// **F24 is the one Copper keeps for itself.** The capture watchdog injects a
/// bare F24 as its liveness probe, and the hook swallows it — but only while the
/// hook is alive, which is exactly the state the probe exists to doubt. In the
/// window between the hook dying and the watchdog noticing, those probes reach
/// the system, and a bare F24 binding would fire on Copper's own diagnostics. A
/// modifier makes it unreachable by the probe, so `Ctrl+F24` and its like stay
/// bindable.
fn bindable_bare(key: Code) -> bool {
	matches!(
		key,
		Code::F13
			| Code::F14 | Code::F15
			| Code::F16 | Code::F17
			| Code::F18 | Code::F19
			| Code::F20 | Code::F21
			| Code::F22 | Code::F23
	)
}

/// The double-tap families Copper offers.
///
/// `Win Win` is deliberately absent: double-tapping it fights the Start menu,
/// which opens on the *release* of a bare Windows key.
fn double_tap_family(token: &str) -> Option<ModifierFamily> {
	match token {
		"SHIFT" => Some(ModifierFamily::Shift),
		"CTRL" | "CONTROL" => Some(ModifierFamily::Control),
		"ALT" => Some(ModifierFamily::Alt),
		_ => None,
	}
}

/// One token of a double-tap spelling, sided or not.
///
/// `LCtrl` and `RShift` are the sided spellings; `Ctrl` and `Shift` are the
/// unsided ones and mean either side, which is what every install predating sided
/// bindings already has written in its `settings.json`. Both must keep parsing:
/// the repair path falls back to the shipped default on anything it cannot read,
/// so a spelling that stopped parsing would silently reset the user's binding.
///
/// The prefix is stripped before the family is looked up rather than the table
/// being written out nine times. That is only safe because no unsided family
/// token begins with `L` or `R` — which is true of Shift, Ctrl, Control and Alt,
/// and is the thing to check before adding a family.
fn double_tap_binding(token: &str) -> Option<WatchedTrigger> {
	let token = token.trim().to_ascii_uppercase();
	let (side, family) = if let Some(rest) = token.strip_prefix('L') {
		(KeySide::Left, rest)
	} else if let Some(rest) = token.strip_prefix('R') {
		(KeySide::Right, rest)
	} else {
		(KeySide::Either, token.as_str())
	};
	Some(WatchedTrigger {
		family: double_tap_family(family)?,
		side,
	})
}

/// Whether a token names the Windows key, on either side or neither.
fn is_windows_key_token(token: &str) -> bool {
	let token = token.trim().to_ascii_uppercase();
	let bare = token
		.strip_prefix('L')
		.or_else(|| token.strip_prefix('R'))
		.unwrap_or(token.as_str());
	matches!(bare, "WIN" | "SUPER" | "COMMAND" | "CMD")
}

/// Accepts both shapes R-Q52 allows, for either role: a bare-modifier double-tap
/// written as `"<Modifier> <Modifier>"`, or any conventional chord.
///
/// One function for both roles rather than two, because the rules turned out to
/// be identical — the hook recognises the same three families whichever action
/// they are pointed at, and the plugin accepts the same chords.
pub fn validate_trigger(text: &str) -> Result<BoundTrigger, ShellError> {
	let text = text.trim();
	let tokens: Vec<&str> = text.split_whitespace().collect();

	if tokens.len() == 2 && tokens[0].eq_ignore_ascii_case(tokens[1]) {
		if let Some(trigger) = double_tap_binding(tokens[0]) {
			return Ok(BoundTrigger::DoubleTap(trigger));
		}
		if is_windows_key_token(tokens[0]) {
			return Err(ShellError::Reserved(
				"Double-tapping the Windows key opens the Start menu, so Copper can't use it."
					.to_owned(),
			));
		}
		return Err(ShellError::InvalidChord(format!(
			"{text} isn't a double-tap Copper recognises. Use Shift, Ctrl or Alt — or one side of \
			 them, like LCtrl."
		)));
	}

	validate_chord(text).map(BoundTrigger::Chord)
}

// --- canonical spelling ------------------------------------------------------

/// One spelling for a chord, in `settings.json` and on screen alike.
///
/// Every token here is one the parser accepts, so what is written to disk always
/// reads back — including `Super`, which is the parser's word for the Windows
/// key. The settings view relabels that one chip `Win`; changing it here instead
/// would produce a `settings.json` the app itself could not parse.
fn display_chord(chord: &Shortcut) -> String {
	let mut parts: Vec<String> = Vec::new();
	// Ctrl, Alt, Shift, Super — Windows' own ordering, not the parser's.
	if chord.mods.contains(Modifiers::CONTROL) {
		parts.push("Ctrl".to_owned());
	}
	if chord.mods.contains(Modifiers::ALT) {
		parts.push("Alt".to_owned());
	}
	if chord.mods.contains(Modifiers::SHIFT) {
		parts.push("Shift".to_owned());
	}
	if chord.mods.contains(Modifiers::SUPER) {
		parts.push("Super".to_owned());
	}
	parts.push(key_label(chord.key));
	parts.join("+")
}

/// `KeyK` and `Digit1` are the W3C `code` spellings the recorder sends and the
/// parser accepts; `K` and `1` are what a person reads. The parser accepts the
/// short forms too, so shortening loses nothing.
fn key_label(key: Code) -> String {
	let name = key.to_string();
	if let Some(letter) = name.strip_prefix("Key") {
		return letter.to_owned();
	}
	if let Some(digit) = name.strip_prefix("Digit") {
		return digit.to_owned();
	}
	name
}

fn family_label(family: ModifierFamily) -> &'static str {
	match family {
		ModifierFamily::Shift => "Shift",
		ModifierFamily::Control => "Ctrl",
		ModifierFamily::Alt => "Alt",
		ModifierFamily::Off => "",
	}
}

/// The stored spelling of a double-tap: `Shift Shift`, `LCtrl LCtrl`, `RAlt
/// RAlt`. This is the canonical writer, and it is what makes the round trip
/// through `settings.json` hold.
fn double_tap_text(trigger: WatchedTrigger) -> String {
	let side = match trigger.side {
		KeySide::Left => "L",
		KeySide::Right => "R",
		KeySide::Either => "",
	};
	let label = format!("{side}{}", family_label(trigger.family));
	format!("{label} {label}")
}

/// A validated binding as the user reads it, whichever shape it is.
fn trigger_text(trigger: &BoundTrigger) -> String {
	match trigger {
		BoundTrigger::DoubleTap(watched) => double_tap_text(*watched),
		BoundTrigger::Chord(chord) => display_chord(chord),
	}
}

/// Turns a validated binding into the registry's record of it, with no side
/// effect on the hook. [`apply_locally`] is what pairs this with the `watch`.
fn binding_of(trigger: BoundTrigger) -> TriggerBinding {
	match trigger {
		BoundTrigger::DoubleTap(watched) => TriggerBinding::DoubleTap {
			text: double_tap_text(watched),
			trigger: watched,
		},
		BoundTrigger::Chord(chord) => TriggerBinding::Chord(Binding {
			text: display_chord(&chord),
			chord,
			registered: false,
			error: None,
		}),
	}
}

// --- the handlers ------------------------------------------------------------

/// Whether this event is the one press the user made.
///
/// The Windows implementation fires `Pressed` on `WM_HOTKEY` and then `Released`
/// once its 50 ms `GetAsyncKeyState` poll sees the key come up, so a handler that
/// does not match on state runs its action twice per press.
fn is_press(event: &ShortcutEvent) -> bool {
	event.state == KeyState::Pressed
}

fn summon_handler(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
	if !is_press(&event) {
		return;
	}
	// A registration a failed `unregister` left behind must not still summon the
	// panel.
	if shortcut.id() != CANONICAL_SUMMON.load(Ordering::Relaxed) {
		return;
	}
	panel::toggle_or_log(app);
}

fn capture_handler(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
	if !is_press(&event) {
		return;
	}
	if shortcut.id() != CANONICAL_CAPTURE.load(Ordering::Relaxed) {
		return;
	}
	capture::request_capture(app);
}

fn register(app: &AppHandle, chord: Shortcut, role: Role) -> Result<(), String> {
	let result = match role {
		Role::Summon => app.global_shortcut().on_shortcut(chord, summon_handler),
		Role::Capture => app.global_shortcut().on_shortcut(chord, capture_handler),
	};
	result.map_err(|err| err.to_string())
}

/// The copy for a refused registration.
///
/// The hedge is the only honest wording available: the plugin stringifies
/// `global_hotkey::Error`, so a chord held by another application is
/// indistinguishable from any other failure. Naming a cause the app has not
/// established would send the user hunting for an app that may not exist.
fn registration_failed_message(text: &str) -> String {
	format!(
		"Windows wouldn't accept {text}. Another app is probably using it. Choose a different \
		 shortcut."
	)
}

fn registration_failed(text: &str) -> ShellError {
	ShellError::RegistrationFailed(registration_failed_message(text))
}

// --- retiring ----------------------------------------------------------------

/// Unregisters `chord`, recording it for a later retry if the OS refuses.
fn retire(app: &AppHandle, registry: &mut Registry, chord: Shortcut) {
	if app.global_shortcut().unregister(chord).is_err() {
		diagnostics::log_error(&format!(
			"[copper] shortcuts: {} would not unregister; it stays claimed but inert until the \
			 next rebind",
			display_chord(&chord)
		));
		registry.stale.push(chord);
	}
}

/// Retries everything a previous retirement could not let go of.
fn retry_stale(app: &AppHandle, registry: &mut Registry) {
	let pending = std::mem::take(&mut registry.stale);
	for chord in pending {
		if app.global_shortcut().unregister(chord).is_err() {
			registry.stale.push(chord);
		}
	}
}

// --- the reported state ------------------------------------------------------

/// Everything the settings view needs in one pull.
///
/// One command rather than an extension of task-003's `get_settings`, so that
/// command's contract and its tests stay untouched. It carries the shipped
/// defaults as well as the current bindings, so Reset renders without the
/// frontend keeping a second copy of the defaults in another language.
///
/// **Startup registration failure is state, not an event.** `setup()` runs before
/// the webview has loaded and Tauri buffers nothing, so a failure emitted there
/// is guaranteed to be missed. It is read here instead.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutState {
	capture: String,
	summon: String,
	defaults: Shortcuts,
	/// A double-tap is live whenever the keyboard hook is; a conventional chord is
	/// live when the OS accepted it. That is now true of summon as well as
	/// capture — before task-020 this field meant only "the OS took the hotkey".
	summon_registered: bool,
	summon_error: Option<String>,
	capture_registered: bool,
	capture_error: Option<String>,
	/// Present only while the keyboard hook is down and the insurance chord is
	/// standing in for that role's double-tap.
	capture_fallback: Option<String>,
	summon_fallback: Option<String>,
}

/// Whether a role's binding can actually be triggered, and what to say if not.
fn live_and_error(registry: &Registry, role: Role) -> (bool, Option<String>) {
	match registry.binding(role) {
		// Nothing to register: the hook either recognises the double-tap or it is
		// not installed, and the fallback chord is what covers the latter. A
		// fallback that could not be had is therefore a failure of *this role*,
		// since between the two of them nothing on the machine can reach it.
		TriggerBinding::DoubleTap { .. } => (
			capture::hook_alive() || registry.fallback(role).chord.is_some(),
			registry.fallback(role).error.clone(),
		),
		TriggerBinding::Chord(binding) => (binding.registered, binding.error.clone()),
	}
}

/// What the tray tooltip is told. Its own function because three call sites need
/// the same answer and a fourth that disagreed would be a tooltip that contradicts
/// the settings view.
fn summon_live(registry: &Registry) -> bool {
	live_and_error(registry, Role::Summon).0
}

fn snapshot(registry: &Registry) -> ShortcutState {
	let (summon_registered, summon_error) = live_and_error(registry, Role::Summon);
	let (capture_registered, capture_error) = live_and_error(registry, Role::Capture);

	ShortcutState {
		capture: registry.capture.text().to_owned(),
		summon: registry.summon.text().to_owned(),
		defaults: Shortcuts {
			capture: DEFAULT_CAPTURE_TRIGGER.to_owned(),
			summon: DEFAULT_SUMMON_SHORTCUT.to_owned(),
		},
		summon_registered,
		summon_error,
		capture_registered,
		capture_error,
		capture_fallback: registry.capture_fallback.chord.as_ref().map(display_chord),
		summon_fallback: registry.summon_fallback.chord.as_ref().map(display_chord),
	}
}

// --- persistence -------------------------------------------------------------

/// Writes both bindings, since they share one `Shortcuts` struct.
fn persist(app: &AppHandle, capture: &str, summon: &str) -> Result<(), ShellError> {
	let patch = SettingsPatch {
		shortcuts: Some(Shortcuts {
			capture: capture.to_owned(),
			summon: summon.to_owned(),
		}),
		..SettingsPatch::default()
	};
	store::commands::patch_settings(app, patch)
		.map(|_| ())
		.map_err(|err| {
			ShellError::Persist(format!(
				"The shortcut works for now, but Copper couldn't save it: {}",
				err.message()
			))
		})
}

/// Writes `text` into `role`'s field, taking the other role's from the registry.
fn persist_role(
	app: &AppHandle,
	registry: &Registry,
	role: Role,
	text: &str,
) -> Result<(), ShellError> {
	match role {
		Role::Capture => persist(app, text, registry.summon.text()),
		Role::Summon => persist(app, registry.capture.text(), text),
	}
}

// --- startup -----------------------------------------------------------------

/// Points the keyboard hook at the persisted bindings, before capture starts.
///
/// Separate from [`install`] and called earlier for one reason: the first
/// double-tap after launch must be judged against the user's binding rather than
/// the compiled-in default, and `capture::start_capture` installs the hook.
///
/// Still named for capture because `lib.rs` calls it by that name, but it now
/// prepares both roles — summon can be a double-tap too, and its recogniser needs
/// pointing at the right family for exactly the same reason.
pub fn prepare_capture(app: &AppHandle) {
	let stored = store::commands::settings(app).shortcuts;
	let mut registry = registry();

	for (role, text) in [
		(Role::Summon, stored.summon.as_str()),
		(Role::Capture, stored.capture.as_str()),
	] {
		let trigger = validate_trigger(text).unwrap_or_else(|err| {
			diagnostics::log_error(&format!(
				"[copper] shortcuts: the stored {} binding {text:?} is not usable ({}); falling \
				 back to {}",
				role.noun(),
				err.message(),
				shipped_text(role)
			));
			shipped_trigger(role)
		});
		apply_locally(&mut registry, role, trigger);
	}
}

/// Puts a validated binding into the registry and the hook's atomic. Cannot fail
/// — which is why the double-tap path may persist before calling it.
fn apply_locally(registry: &mut Registry, role: Role, trigger: BoundTrigger) {
	capture::watch(
		role.hook_role(),
		match trigger {
			BoundTrigger::DoubleTap(watched) => watched,
			// The two are mutually exclusive at runtime: a conventional chord means
			// this recogniser has no double-tap to recognise.
			BoundTrigger::Chord(_) => WatchedTrigger::OFF,
		},
	);
	*registry.binding_mut(role) = binding_of(trigger);
}

/// Registers everything the OS has to know about, after capture has started.
///
/// Returns nothing and propagates nothing. A shortcut that will not register must
/// leave the app running with a working tray: for an app that starts hidden,
/// returning `Err` from `setup()` is as fatal as panicking, and the tray is the
/// recovery path this failure is reported through.
///
/// It reads the bindings [`prepare_capture`] already validated rather than
/// re-reading the store: two parses of one string is two chances to disagree, and
/// the hook is by this point already watching whatever the first one decided.
pub fn install(app: &AppHandle) {
	let mut registry = registry();

	for role in ROLES {
		if let TriggerBinding::Chord(binding) = registry.binding_mut(role) {
			let chord = binding.chord;
			match register(app, chord, role) {
				Ok(()) => {
					binding.registered = true;
					canonical(role).store(chord.id(), Ordering::Relaxed);
				}
				Err(err) => {
					diagnostics::log_error(&format!(
						"[copper] shortcuts: the {} chord {} could not be registered: {err}",
						role.noun(),
						binding.text
					));
					binding.error = Some(registration_failed_message(&binding.text));
				}
			}
		}
		ensure_fallback(app, &mut registry, role);
	}

	tray::report_summon(app, summon_live(&registry));
}

/// Registers a role's insurance chord when — and only when — the keyboard hook is
/// down and that role is bound to a double-tap the hook can no longer recognise.
fn ensure_fallback(app: &AppHandle, registry: &mut Registry, role: Role) {
	let wanted = matches!(registry.binding(role), TriggerBinding::DoubleTap { .. })
		&& !capture::hook_alive();
	// Copied out before the match rather than read inside it: the arms take the
	// registry mutably, and a borrow held by the scrutinee would outlive them.
	let held = registry.fallback(role).chord;

	match (wanted, held) {
		(true, None) => match Shortcut::from_str(fallback_chord_text(role)) {
			Ok(chord) => match register(app, chord, role) {
				Ok(()) => {
					canonical(role).store(chord.id(), Ordering::Relaxed);
					let fallback = registry.fallback_mut(role);
					fallback.chord = Some(chord);
					fallback.error = None;
					diagnostics::log(&format!(
						"[copper] shortcuts: the keyboard hook is unavailable; {} is reachable \
						 through {}",
						role.noun(),
						fallback_chord_text(role)
					));
				}
				Err(err) => {
					// Recorded, not merely logged. Logging alone left the settings view
					// saying the binding was fine while nothing on the machine could
					// reach it.
					registry.fallback_mut(role).error = Some(fallback_unavailable(role));
					diagnostics::log_error(&format!(
						"[copper] shortcuts: the fallback {} chord could not be registered: {err}",
						role.noun()
					));
				}
			},
			Err(err) => {
				registry.fallback_mut(role).error = Some(fallback_unavailable(role));
				diagnostics::log_error(&format!(
					"[copper] shortcuts: the fallback {} chord is not parseable: {err}",
					role.noun()
				));
			}
		},
		(false, Some(chord)) => {
			let fallback = registry.fallback_mut(role);
			fallback.chord = None;
			fallback.error = None;
			retire(app, registry, chord);
		}
		// Nothing wanted and nothing held: any complaint left over from a previous
		// pass no longer describes anything.
		(false, None) => registry.fallback_mut(role).error = None,
		(true, Some(_)) => {}
	}
}

/// Re-decides both insurance chords after the keyboard hook changed hands.
///
/// Called by the capture watchdog, from a thread of its own — never from the
/// main thread, per the module note: this takes the registry lock and
/// [`ensure_fallback`] registers through the plugin, which blocks on the main
/// thread from anywhere else.
///
/// It reads the hook's liveness through `capture::hook_alive`, the same canonical
/// atomic the startup path reads, rather than being told what to do. A caller
/// that passed the answer in could disagree with the flag, and the two would then
/// have to be kept in step.
///
/// Checked against teardown **after** the lock is taken as well as before the
/// thread was spawned. Waiting for this lock can take arbitrarily long — a rebind
/// holds it across a main-thread round trip — and exit may well have begun in the
/// meantime, at which point registering a chord is at best pointless and at worst
/// holds the lock across [`shutdown`]'s `try_lock` and costs it every retirement.
///
/// The tray is told **after** the guard is dropped. A hook that came back or went
/// away changes whether a double-tap summon works, so the tooltip has to be
/// revisited too — but `set_tooltip` reaches the main thread, and doing it under
/// the lock would put a main-thread round trip inside the one critical section
/// this module's header rule is about.
pub fn revisit_fallback(app: &AppHandle) {
	let live = {
		let mut registry = registry();
		if crate::shutting_down() {
			return;
		}
		for role in ROLES {
			ensure_fallback(app, &mut registry, role);
		}
		summon_live(&registry)
	};
	tray::report_summon(app, live);
}

/// Best-effort tidy-up at exit.
///
/// `try_lock` rather than a blocking one: Windows releases a process's hotkey
/// registrations when it exits, so the worst case of skipping this is nothing at
/// all, while blocking here would trade a tidy exit for a hung one.
pub fn shutdown(app: &AppHandle) {
	let Some(mut registry) = try_registry() else {
		return;
	};
	retry_stale(app, &mut registry);

	for role in ROLES {
		if let TriggerBinding::Chord(binding) = registry.binding(role) {
			if binding.registered {
				let _ = app.global_shortcut().unregister(binding.chord);
			}
		}
		// Cleared, not merely unregistered. `ensure_fallback` decides what to do from
		// this field, so a `Some` left behind over a registration that is gone reads
		// as "the chord is already up" and does nothing — which since the watchdog
		// began calling it in the background is a reachable state rather than a
		// theoretical one, and its symptom is a binding with no trigger at all.
		if let Some(chord) = registry.fallback_mut(role).chord.take() {
			let _ = app.global_shortcut().unregister(chord);
			canonical(role).store(NO_CHORD, Ordering::Relaxed);
		}
	}
}

// --- the rebind protocol -----------------------------------------------------

/// Rebinds the summon binding.
pub fn set_summon(app: &AppHandle, text: &str) -> Result<ShortcutState, ShellError> {
	let mut registry = registry();
	set_trigger_locked(app, &mut registry, Role::Summon, text)?;
	Ok(snapshot(&registry))
}

/// Rebinds the capture binding.
pub fn set_capture(app: &AppHandle, text: &str) -> Result<ShortcutState, ShellError> {
	let mut registry = registry();
	set_trigger_locked(app, &mut registry, Role::Capture, text)?;
	Ok(snapshot(&registry))
}

/// The body, taking a guard the caller already holds.
///
/// `commit_recording` needs the token check, the rebind and the lease restore to
/// happen under **one** acquisition. Releasing the lock between them made the
/// token a check-then-act: a superseding `begin` landing in that window left the
/// stale commit free to write and to close the newer session.
fn set_trigger_locked(
	app: &AppHandle,
	registry: &mut Registry,
	role: Role,
	text: &str,
) -> Result<(), ShellError> {
	let wanted = validate_trigger(text)?;
	retry_stale(app, registry);

	// Cross-role exclusivity compares the **configured** bindings and deliberately
	// does not consult `registered`. Every rebind arrives inside a recording lease,
	// which is exactly when everything is unregistered — so a guard that asked
	// whether the other binding was live could never fire, the OS would accept the
	// duplicate, and `persist` would write the same binding into both fields. The
	// next launch then registers summon first and capture fails for good.
	let other = role.other();
	if claims(registry, other, &wanted) {
		return Err(ShellError::Reserved(format!(
			"{} is already Copper's {} shortcut. Choose a different one.",
			trigger_text(&wanted),
			other.noun()
		)));
	}

	match wanted {
		// A double-tap swap **persists first**: pointing a hook selector at another
		// binding cannot fail, so writing first means the file and the runtime can
		// never disagree.
		BoundTrigger::DoubleTap(trigger) => set_double_tap(app, registry, role, trigger)?,
		// A conventional chord has a registration that can fail, so it registers the
		// new one before retiring the old: the plugin offers no atomic replace and
		// the two orderings fail asymmetrically. This one can briefly leave two
		// chords bound, which is recoverable; the other can leave none, which is the
		// lockout the whole protocol exists to prevent. Persistence happens in the
		// *middle*, while the old chord is still live — done afterwards, a write
		// failure would leave the runtime on the new chord and the file on the old.
		BoundTrigger::Chord(chord) => set_chord(app, registry, role, chord)?,
	}

	if role == Role::Summon {
		tray::report_summon(app, summon_live(registry));
	}
	Ok(())
}

fn set_double_tap(
	app: &AppHandle,
	registry: &mut Registry,
	role: Role,
	trigger: WatchedTrigger,
) -> Result<(), ShellError> {
	let text = double_tap_text(trigger);

	// Not an early return. A re-submit or a Reset that lands on the binding
	// already stored still has work to do below: the insurance chord may be
	// stored-but-dead, and returning here is what made it unretryable.
	if registry.binding(role).text() != text {
		persist_role(app, registry, role, &text)?;
		let previous = registry.binding(role).clone();
		apply_locally(registry, role, BoundTrigger::DoubleTap(trigger));
		if let TriggerBinding::Chord(binding) = previous {
			canonical(role).store(NO_CHORD, Ordering::Relaxed);
			if binding.registered {
				retire(app, registry, binding.chord);
			}
		}
	}

	// The hook may be down, in which case the double-tap needs the insurance chord
	// that a conventional binding did not.
	ensure_fallback(app, registry, role);
	Ok(())
}

fn set_chord(
	app: &AppHandle,
	registry: &mut Registry,
	role: Role,
	chord: Shortcut,
) -> Result<(), ShellError> {
	let text = display_chord(&chord);

	// Same-role idempotency compares against what is *actually registered*.
	// Re-submitting the live binding is an idempotent success; re-submitting one
	// that is stored but not registered — the startup-failure case — is a retry and
	// has to fall through.
	if let TriggerBinding::Chord(binding) = registry.binding(role) {
		if binding.chord == chord && binding.registered {
			return Ok(());
		}
	}

	register(app, chord, role).map_err(|err| {
		diagnostics::log_error(&format!("[copper] shortcuts: {text} was refused: {err}"));
		registration_failed(&text)
	})?;

	if let Err(err) = persist_role(app, registry, role, &text) {
		// Nothing durable happened, so nothing may stay changed. The old binding was
		// never retired and is still the live one. Rolled back through `retire`
		// rather than a bare `unregister`, so a chord the OS refuses to give up is
		// recorded and retried instead of left claimed by nobody.
		retire(app, registry, chord);
		return Err(err);
	}

	let previous = registry.binding(role).clone();
	apply_locally(registry, role, BoundTrigger::Chord(chord));
	if let TriggerBinding::Chord(binding) = registry.binding_mut(role) {
		binding.registered = true;
	}
	canonical(role).store(chord.id(), Ordering::Relaxed);

	// Only now, and only if it is a different chord: after a startup-failure retry
	// the "previous" chord *is* this one, and retiring it would unregister what was
	// just registered.
	if let TriggerBinding::Chord(binding) = previous {
		if binding.registered && binding.chord != chord {
			retire(app, registry, binding.chord);
		}
	}

	// The insurance chord exists to keep a *double-tap* reachable; a chord binding
	// does not need it and having both would claim a hotkey for nothing.
	ensure_fallback(app, registry, role);
	Ok(())
}

/// Whether two double-taps would answer to the same gesture.
///
/// Same family, and sides that overlap — which is not the same as sides that are
/// equal. `Ctrl Ctrl` is the unsided spelling and means *either* side, so it
/// collides with `LCtrl LCtrl` and with `RCtrl RCtrl` alike, while those two do
/// not collide with each other. Treating this as equality would let the user bind
/// summon to `Ctrl Ctrl` and capture to `LCtrl LCtrl` and then wonder which of
/// the two their left Ctrl does.
fn double_taps_collide(one: WatchedTrigger, other: WatchedTrigger) -> bool {
	one.family == other.family && one.side.matches(other.side)
}

/// Whether the binding for `role` already claims `wanted`, live or not.
fn claims(registry: &Registry, role: Role, wanted: &BoundTrigger) -> bool {
	match (registry.binding(role), wanted) {
		(TriggerBinding::Chord(binding), BoundTrigger::Chord(chord)) => binding.chord == *chord,
		// The insurance chord is as much a claim as a binding: it is registered, it
		// fires this role's handler, and letting the other role take it would mean
		// one keystroke doing two things — or, once the hook came back and the
		// insurance was retired, silently doing neither.
		(TriggerBinding::DoubleTap { .. }, BoundTrigger::Chord(chord)) => {
			registry.fallback(role).chord == Some(*chord)
		}
		(TriggerBinding::DoubleTap { trigger, .. }, BoundTrigger::DoubleTap(wanted)) => {
			double_taps_collide(*trigger, *wanted)
		}
		// A chord and a double-tap are different gestures on different machinery.
		(TriggerBinding::Chord(_), BoundTrigger::DoubleTap(_)) => false,
	}
}

// --- the recording lease -----------------------------------------------------

/// The lease is owned by Rust, not by the frontend.
///
/// A registered chord arrives as `WM_HOTKEY` at the registering message window
/// and never reaches the webview's `keydown`, so the live chords have to come
/// down while the user is recording a new one. Leaving restoration to the
/// frontend leaks them on every path the frontend does not control — navigating
/// back, the panel being hidden from the tray, unmount, a WebView reload, a
/// failed IPC call — and because the Rust process stays alive throughout, the
/// user is then left with no summon shortcut until they restart the app. That is
/// the same lockout this task exists to prevent, arrived at from the other side.
///
/// **The hook is muted as well as the chords unregistered.** A double-tap is not
/// the plugin's to unregister, so before task-020 a live capture double-tap fired
/// while the user was recording over it. For summon that is not merely untidy: a
/// double-tap summon toggles the panel, hiding the panel calls
/// [`cancel_recording_off_thread`], and the session the user just opened ends
/// itself with no explanation.
pub fn begin_recording(app: &AppHandle) -> Result<u64, ShellError> {
	let mut registry = registry();
	let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);

	// A second `begin` supersedes the first rather than stranding it: the
	// suspension is already in place, so only the token moves and the record of
	// what to put back survives untouched.
	if let Some(lease) = &mut registry.lease {
		lease.token = token;
		arm_watchdog(app, token);
		return Ok(token);
	}

	// First, and unconditionally: it cannot fail, and it costs nothing to undo.
	// The chords below can fail to come down, and the `blocked` path relies on the
	// restore putting this back with them.
	capture::mute(true);

	// Each outcome is recorded rather than assumed. A chord the OS refuses to give
	// up is still intercepting `WM_HOTKEY`, so the keystroke never reaches the
	// webview — and a session opened over it is a recording box that silently
	// ignores the very chord the user is trying to replace.
	let mut suspended = Lease {
		token,
		summon: false,
		capture: false,
		summon_fallback: false,
		capture_fallback: false,
	};
	let mut blocked = false;

	for role in ROLES {
		// `registered` means "live right now", so it goes false only where the chord
		// actually came down. Any other reading breaks the setters: re-submitting the
		// chord that is currently bound would look unchanged, return early, and leave
		// it unregistered when the lease is released.
		if let TriggerBinding::Chord(binding) = registry.binding_mut(role) {
			if binding.registered {
				if app.global_shortcut().unregister(binding.chord).is_ok() {
					binding.registered = false;
					*suspended.chord_mut(role) = true;
					canonical(role).store(NO_CHORD, Ordering::Relaxed);
				} else {
					blocked = true;
				}
			}
		}
		let insurance = registry.fallback(role).chord;
		if let Some(chord) = insurance {
			if app.global_shortcut().unregister(chord).is_ok() {
				*suspended.fallback_mut(role) = true;
				canonical(role).store(NO_CHORD, Ordering::Relaxed);
			} else {
				blocked = true;
			}
		}
	}

	registry.lease = Some(suspended);
	LEASE_OPEN.store(true, Ordering::Relaxed);

	if blocked {
		// Put back whatever did come down, then refuse. Reusing the restore path
		// rather than unwinding by hand is what keeps the two in step.
		restore_lease(app, &mut registry, None);
		diagnostics::log_error(
			"[copper] shortcuts: a live chord would not unregister, so no recording session was \
			 opened",
		);
		return Err(ShellError::RegistrationFailed(
			"Copper couldn't free its current shortcuts to record over them. Try again."
				.to_owned(),
		));
	}

	arm_watchdog(app, token);
	Ok(token)
}

/// Takes the lease back if the frontend never did.
fn arm_watchdog(app: &AppHandle, token: u64) {
	let app = app.clone();
	std::thread::spawn(move || {
		std::thread::sleep(LEASE_WATCHDOG);
		let mut registry = registry();
		if registry.lease.as_ref().is_some_and(|lease| lease.token == token) {
			diagnostics::log_error(
				"[copper] shortcuts: a recording session was abandoned; restoring the previous \
				 bindings",
			);
			restore_lease(&app, &mut registry, None);
		}
	});
}

/// Puts back everything [`begin_recording`] suspended, except `replaced` — which
/// the caller has just registered anew and must not have unregistered under it.
fn restore_lease(app: &AppHandle, registry: &mut Registry, replaced: Option<Role>) {
	let Some(lease) = registry.lease.take() else {
		return;
	};
	LEASE_OPEN.store(false, Ordering::Relaxed);
	// Unconditionally, and regardless of `replaced`: the mute is not per-role, and
	// a role whose binding was just replaced needs its recogniser back as much as
	// the other one does.
	capture::mute(false);

	let mut summon_failed = false;

	for role in ROLES {
		if replaced == Some(role) {
			continue;
		}

		if lease.chord(role) {
			if let TriggerBinding::Chord(binding) = registry.binding_mut(role) {
				let chord = binding.chord;
				match register(app, chord, role) {
					Ok(()) => {
						binding.registered = true;
						canonical(role).store(chord.id(), Ordering::Relaxed);
					}
					Err(err) => {
						// The one failure a user cannot recover from without being told,
						// so it reaches both surfaces the startup failure reaches.
						binding.error = Some(registration_failed_message(&binding.text));
						diagnostics::log_error(&format!(
							"[copper] shortcuts: the {} chord could not be restored after \
							 recording: {err}",
							role.noun()
						));
						summon_failed |= role == Role::Summon;
					}
				}
			}
		}

		if lease.fallback(role) {
			let insurance = registry.fallback(role).chord;
			if let Some(chord) = insurance {
				if register(app, chord, role).is_ok() {
					canonical(role).store(chord.id(), Ordering::Relaxed);
				} else {
					// Forgotten rather than kept, so `ensure_fallback` treats it as absent
					// and registers it again on the next pass. Held here, it would look
					// live forever while claiming nothing.
					let fallback = registry.fallback_mut(role);
					fallback.chord = None;
					fallback.error = Some(fallback_unavailable(role));
					summon_failed |= role == Role::Summon;
				}
			}
		}
	}

	if summon_failed {
		tray::report_summon(app, false);
	}
}

/// Cancels whatever session is open. Idempotent, and deliberately not fussy about
/// the token: a caller asking to stop recording must never be able to leave the
/// chords suspended because it quoted a token that had already been superseded.
pub fn cancel_recording(app: &AppHandle) -> ShortcutState {
	let mut registry = registry();
	restore_lease(app, &mut registry, None);
	snapshot(&registry)
}

/// Applies a recorded binding to one of the two roles.
///
/// Rejects a stale token — unlike cancel, this one *writes*, and applying a
/// binding recorded in a session the user has already left is a change they did
/// not ask for.
pub fn commit_recording(
	app: &AppHandle,
	token: u64,
	target: &str,
	chord: &str,
) -> Result<ShortcutState, ShellError> {
	// One acquisition for the token check, the rebind and the restore. Released
	// between any two of them, the token becomes a check-then-act: a superseding
	// `begin` landing in the gap would leave this stale commit free to write, and
	// to close the newer session on its way out.
	let mut registry = registry();

	match &registry.lease {
		Some(lease) if lease.token == token => {}
		_ => {
			return Err(ShellError::StaleToken(
				"That recording has already finished. Try again.".to_owned(),
			))
		}
	}

	// Validated inside the session rather than before it. Returning early on an
	// unrecognised target would leave every binding suspended until the watchdog or
	// a panel hide noticed — a nonsense argument must not cost the user their
	// shortcuts.
	let role = match target {
		"summon" => Role::Summon,
		"capture" => Role::Capture,
		other => {
			let message = format!("{other} is not a shortcut Copper can rebind.");
			restore_lease(app, &mut registry, None);
			return Err(ShellError::Invalid(message));
		}
	};

	let outcome = set_trigger_locked(app, &mut registry, role, chord);

	// Whatever happened, the session is over: on success the other bindings come
	// back and the replaced one is already live; on failure everything comes back
	// exactly as it was, which is what makes a refused binding a no-op rather than
	// a lockout.
	restore_lease(app, &mut registry, outcome.is_ok().then_some(role));
	outcome.map(|()| snapshot(&registry))
}

/// The main-thread-safe way to end a session — see the module note on the lock.
///
/// Every panel hide calls this, and almost none of them have a session to end, so
/// the flag is checked before a thread is spawned rather than inside it.
pub fn cancel_recording_off_thread(app: &AppHandle) {
	if !LEASE_OPEN.load(Ordering::Relaxed) {
		return;
	}
	let app = app.clone();
	std::thread::spawn(move || {
		cancel_recording(&app);
	});
}

// --- commands ----------------------------------------------------------------

type Reply<T> = std::result::Result<T, ShellError>;

#[tauri::command]
pub async fn get_shortcut_state() -> Reply<ShortcutState> {
	let registry = registry();
	Ok(snapshot(&registry))
}

#[tauri::command]
pub async fn set_summon_shortcut(chord: String, app: AppHandle) -> Reply<ShortcutState> {
	set_summon(&app, &chord)
}

#[tauri::command]
pub async fn set_capture_trigger(trigger: String, app: AppHandle) -> Reply<ShortcutState> {
	set_capture(&app, &trigger)
}

#[tauri::command]
pub async fn begin_shortcut_recording(app: AppHandle) -> Reply<u64> {
	begin_recording(&app)
}

#[tauri::command]
pub async fn commit_shortcut_recording(
	token: u64,
	target: String,
	chord: String,
	app: AppHandle,
) -> Reply<ShortcutState> {
	commit_recording(&app, token, &target, &chord)
}

#[tauri::command]
pub async fn cancel_shortcut_recording(app: AppHandle) -> Reply<ShortcutState> {
	Ok(cancel_recording(&app))
}

// --- tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
	use super::*;
	use copper_core::store::settings::Settings;

	/// The four sided spellings a test reaches for most.
	fn double_tap(family: ModifierFamily, side: KeySide) -> BoundTrigger {
		BoundTrigger::DoubleTap(WatchedTrigger { family, side })
	}

	#[test]
	fn the_defaults_match_the_ones_the_store_ships() {
		// Two hardcoded copies of one default is the drift this catches.
		let shipped = Settings::default().shortcuts;
		assert_eq!(shipped.summon, DEFAULT_SUMMON_SHORTCUT);
		assert_eq!(shipped.capture, DEFAULT_CAPTURE_TRIGGER);
	}

	#[test]
	fn the_built_defaults_equal_the_parsed_ones() {
		// `shipped_trigger` builds rather than parses, so that no release-build path
		// can abort on an `expect`. This is where the two spellings are held
		// together — for both roles, since either may now be either shape.
		assert_eq!(
			shipped_trigger(Role::Summon),
			BoundTrigger::Chord(Shortcut::from_str(DEFAULT_SUMMON_SHORTCUT).unwrap())
		);
		assert_eq!(
			shipped_trigger(Role::Capture),
			validate_trigger(DEFAULT_CAPTURE_TRIGGER).unwrap()
		);
		// And the text the registry starts with is the text the store ships.
		let registry = Registry::shipped();
		assert_eq!(registry.summon.text(), DEFAULT_SUMMON_SHORTCUT);
		assert_eq!(registry.capture.text(), DEFAULT_CAPTURE_TRIGGER);
	}

	#[test]
	fn valid_chords_parse() {
		assert!(validate_chord("Ctrl+Shift+Space").is_ok());
		// Maps to Control on Windows.
		let mapped = validate_chord("CommandOrControl+K").unwrap();
		assert!(mapped.mods.contains(Modifiers::CONTROL));
		assert_eq!(mapped.key, Code::KeyK);
		assert!(validate_chord("Ctrl+Alt+C").is_ok());
	}

	#[test]
	fn modifier_only_input_is_named_as_such() {
		// The parser calls this "invalid hotkey format", which is the same message
		// it gives for genuine nonsense — two different mistakes to make.
		assert_eq!(validate_chord("Shift+Ctrl").unwrap_err().kind(), "modifier-only");
		assert_eq!(validate_chord("Shift").unwrap_err().kind(), "modifier-only");
		// A sided modifier on its own is the same mistake, and gets the same answer
		// rather than "Copper couldn't read that".
		assert_eq!(validate_chord("LShift").unwrap_err().kind(), "modifier-only");
		assert_eq!(validate_trigger("RCtrl").unwrap_err().kind(), "modifier-only");
	}

	#[test]
	fn malformed_input_is_rejected_before_the_os_sees_it() {
		// Modifiers must precede the main key.
		assert_eq!(validate_chord("Ctrl+KeyQ+Shift").unwrap_err().kind(), "invalid-chord");
		assert_eq!(validate_chord("Nonsense").unwrap_err().kind(), "invalid-chord");
		assert_eq!(validate_chord("").unwrap_err().kind(), "invalid-chord");
	}

	#[test]
	fn a_single_key_with_no_modifier_is_refused() {
		// Binding one globally takes that key away from every application on the
		// machine, so the recorder refuses it too — this is the half that cannot be
		// bypassed.
		for chord in ["K", "Space", "F5", "F12", "ArrowUp", "Digit1"] {
			let err = validate_chord(chord).unwrap_err();
			assert_eq!(err.kind(), "invalid-chord", "{chord} was accepted bare");
		}
	}

	#[test]
	fn the_high_function_keys_are_the_one_bare_exception() {
		// F13–F23 exist for exactly this: no keyboard emits them by accident and
		// nothing else is listening for them.
		for chord in ["F13", "F19", "F23"] {
			assert!(validate_chord(chord).is_ok(), "{chord} was refused");
		}
		// And they are still fine with modifiers, which is the ordinary case.
		assert!(validate_chord("Ctrl+F13").is_ok());
	}

	#[test]
	fn bare_f24_is_reserved_for_the_watchdogs_probe() {
		// The capture watchdog injects a bare F24 to ask whether its hook is still
		// alive. The hook swallows it — but only while the hook is alive, which is
		// precisely the state the probe exists to doubt, so a bare F24 binding would
		// fire on Copper's own diagnostics in the window this is all about.
		assert_eq!(validate_chord("F24").unwrap_err().kind(), "invalid-chord");
		// A modifier puts it out of the probe's reach, so those stay bindable.
		assert!(validate_chord("Ctrl+F24").is_ok());
		assert!(validate_chord("Ctrl+Alt+Shift+F24").is_ok());
	}

	/// The guard that was inert. Both rebinds arrive inside a recording lease,
	/// which is precisely when every binding is unregistered — so a check that
	/// consulted `registered` could never fire, and the same chord ended up
	/// persisted into both fields.
	#[test]
	fn cross_role_exclusivity_ignores_whether_the_other_binding_is_live() {
		let mut registry = Registry::shipped();
		let chord = validate_chord("Ctrl+Alt+C").unwrap();
		registry.capture = TriggerBinding::Chord(Binding {
			text: display_chord(&chord),
			chord,
			// Suspended, exactly as `begin_recording` leaves it.
			registered: false,
			error: None,
		});

		assert!(
			claims(&registry, Role::Capture, &BoundTrigger::Chord(chord)),
			"a suspended capture chord still claims its binding"
		);
		let summon_chord = match &registry.summon {
			TriggerBinding::Chord(binding) => binding.chord,
			TriggerBinding::DoubleTap { .. } => unreachable!("summon ships as a chord"),
		};
		assert!(!claims(&registry, Role::Capture, &BoundTrigger::Chord(summon_chord)));

		// And the mirror, for a summon binding that is stored but not registered.
		if let TriggerBinding::Chord(binding) = &mut registry.summon {
			binding.registered = false;
		}
		assert!(claims(&registry, Role::Summon, &BoundTrigger::Chord(summon_chord)));
	}

	#[test]
	fn a_double_tap_binding_claims_only_its_own_insurance_chord() {
		let mut registry = Registry::shipped();
		let capture_insurance = Shortcut::from_str(FALLBACK_CAPTURE_CHORD).unwrap();
		let summon_insurance = Shortcut::from_str(FALLBACK_SUMMON_CHORD).unwrap();

		assert!(!claims(&registry, Role::Capture, &BoundTrigger::Chord(capture_insurance)));

		registry.capture_fallback.chord = Some(capture_insurance);
		assert!(claims(&registry, Role::Capture, &BoundTrigger::Chord(capture_insurance)));
		// Not the other role's, and not any other chord.
		assert!(!claims(&registry, Role::Capture, &BoundTrigger::Chord(summon_insurance)));

		// Summon ships as a chord, so it claims its chord and never an insurance one.
		registry.summon_fallback.chord = Some(summon_insurance);
		assert!(!claims(&registry, Role::Summon, &BoundTrigger::Chord(summon_insurance)));
		// Made a double-tap, it does.
		apply_locally(
			&mut registry,
			Role::Summon,
			double_tap(ModifierFamily::Control, KeySide::Left),
		);
		assert!(claims(&registry, Role::Summon, &BoundTrigger::Chord(summon_insurance)));
	}

	#[test]
	fn two_double_taps_collide_when_their_sides_overlap() {
		// The rule that is not equality: the unsided spelling means *either* side,
		// so it overlaps both sided ones while those two do not overlap each other.
		let unsided = WatchedTrigger::unsided(ModifierFamily::Control);
		let left = WatchedTrigger {
			family: ModifierFamily::Control,
			side: KeySide::Left,
		};
		let right = WatchedTrigger {
			family: ModifierFamily::Control,
			side: KeySide::Right,
		};
		let left_shift = WatchedTrigger {
			family: ModifierFamily::Shift,
			side: KeySide::Left,
		};

		assert!(double_taps_collide(unsided, unsided));
		assert!(double_taps_collide(unsided, left));
		assert!(double_taps_collide(left, unsided));
		assert!(double_taps_collide(unsided, right));
		assert!(double_taps_collide(left, left));
		// The whole point of the sided spellings: the two sides are separable.
		assert!(!double_taps_collide(left, right));
		// A different family never collides, whatever the sides say.
		assert!(!double_taps_collide(left, left_shift));
		assert!(!double_taps_collide(unsided, left_shift));
	}

	#[test]
	fn a_double_tap_and_a_chord_never_claim_each_other() {
		// Different gestures on different machinery — the hook sees one, the plugin
		// the other — so neither can be taken by the other's binding.
		let mut registry = Registry::shipped();
		apply_locally(
			&mut registry,
			Role::Capture,
			double_tap(ModifierFamily::Control, KeySide::Left),
		);
		let chord = validate_chord("Ctrl+Alt+C").unwrap();
		assert!(!claims(&registry, Role::Capture, &BoundTrigger::Chord(chord)));
		assert!(!claims(
			&registry,
			Role::Summon,
			&double_tap(ModifierFamily::Control, KeySide::Left)
		));

		// And the collision that *is* real: summon on the unsided spelling of the
		// family capture holds one side of.
		apply_locally(
			&mut registry,
			Role::Summon,
			BoundTrigger::DoubleTap(WatchedTrigger::unsided(ModifierFamily::Control)),
		);
		assert!(claims(
			&registry,
			Role::Summon,
			&double_tap(ModifierFamily::Control, KeySide::Left)
		));
	}

	#[test]
	fn combinations_windows_never_delivers_are_reserved() {
		for chord in ["Super+L", "Alt+Tab", "Ctrl+Alt+Delete", "PrintScreen"] {
			let err = validate_chord(chord).unwrap_err();
			assert_eq!(err.kind(), "reserved", "{chord} was not reserved");
			assert!(!err.message().is_empty());
		}
		// A Windows-key chord that also carries a combining modifier is fine —
		// Windows only keeps the bare ones for itself.
		assert!(validate_chord("Ctrl+Super+K").is_ok());
	}

	#[test]
	fn both_shapes_r_q52_allows_are_accepted_for_either_role() {
		assert_eq!(
			validate_trigger("Shift Shift").unwrap(),
			double_tap(ModifierFamily::Shift, KeySide::Either)
		);
		assert_eq!(
			validate_trigger("Ctrl Ctrl").unwrap(),
			double_tap(ModifierFamily::Control, KeySide::Either)
		);
		assert_eq!(
			validate_trigger("Alt Alt").unwrap(),
			double_tap(ModifierFamily::Alt, KeySide::Either)
		);
		// Case is not part of the binding.
		assert_eq!(
			validate_trigger("control control").unwrap(),
			double_tap(ModifierFamily::Control, KeySide::Either)
		);
		assert!(matches!(
			validate_trigger("Ctrl+Alt+C").unwrap(),
			BoundTrigger::Chord(_)
		));
	}

	#[test]
	fn the_sided_spellings_parse_and_keep_their_side() {
		// These are what the recorder now writes when the user taps one physical
		// key, and what `settings.json` then holds. A spelling that stopped parsing
		// would fall back to the shipped default and silently lose the binding.
		for (text, family, side) in [
			("LShift LShift", ModifierFamily::Shift, KeySide::Left),
			("RShift RShift", ModifierFamily::Shift, KeySide::Right),
			("LCtrl LCtrl", ModifierFamily::Control, KeySide::Left),
			("RCtrl RCtrl", ModifierFamily::Control, KeySide::Right),
			("LAlt LAlt", ModifierFamily::Alt, KeySide::Left),
			("RAlt RAlt", ModifierFamily::Alt, KeySide::Right),
			// The long spelling of Control, sided, since the parser takes it unsided.
			("LControl LControl", ModifierFamily::Control, KeySide::Left),
		] {
			assert_eq!(
				validate_trigger(text).unwrap(),
				double_tap(family, side),
				"{text} did not parse as itself"
			);
		}
		// And case is no more part of a sided binding than an unsided one.
		assert_eq!(
			validate_trigger("lctrl LCTRL").unwrap(),
			double_tap(ModifierFamily::Control, KeySide::Left)
		);
	}

	#[test]
	fn a_double_tap_of_two_different_keys_is_not_a_binding() {
		// Including two sides of one family: `LCtrl RCtrl` is a gesture nothing
		// recognises, and reading it as `Ctrl Ctrl` would be inventing a binding.
		for text in ["LCtrl RCtrl", "Shift Ctrl", "LShift RShift"] {
			assert_eq!(
				validate_trigger(text).unwrap_err().kind(),
				"invalid-chord",
				"{text} was accepted"
			);
		}
	}

	#[test]
	fn the_shapes_the_hook_cannot_service_are_refused() {
		// Double-tapping Win opens the Start menu on the release of a bare press —
		// on either side of the keyboard, so both spellings say so.
		for text in ["Win Win", "LWin LWin", "RWin RWin", "Super Super"] {
			assert_eq!(
				validate_trigger(text).unwrap_err().kind(),
				"reserved",
				"{text} was not reserved"
			);
		}
		// A bare modifier is not a double-tap.
		assert_eq!(validate_trigger("Shift").unwrap_err().kind(), "modifier-only");
	}

	#[test]
	fn a_chord_round_trips_through_its_display_spelling() {
		// What is written to `settings.json` has to read back, or a rebind survives
		// exactly until the next launch.
		for text in [
			"Ctrl+Shift+Space",
			"Ctrl+Alt+C",
			"Alt+Shift+F12",
			"Ctrl+Super+K",
			"Ctrl+Alt+Digit1",
			"Shift+Alt+ArrowUp",
		] {
			let chord = validate_chord(text).unwrap();
			let rendered = display_chord(&chord);
			assert_eq!(
				validate_chord(&rendered).unwrap(),
				chord,
				"{text} rendered as {rendered}, which does not read back"
			);
		}
	}

	#[test]
	fn every_double_tap_round_trips_through_its_stored_spelling() {
		// The same guarantee for the other shape, over every binding the recorder
		// can produce rather than a sample — this is what a `settings.json` written
		// by one build has to survive being read by the next.
		for family in [
			ModifierFamily::Shift,
			ModifierFamily::Control,
			ModifierFamily::Alt,
		] {
			for side in [KeySide::Either, KeySide::Left, KeySide::Right] {
				let trigger = WatchedTrigger { family, side };
				let rendered = double_tap_text(trigger);
				assert_eq!(
					validate_trigger(&rendered).unwrap(),
					BoundTrigger::DoubleTap(trigger),
					"{trigger:?} rendered as {rendered:?}, which does not read back"
				);
			}
		}
	}

	#[test]
	fn the_display_spelling_is_the_one_a_person_reads() {
		let chord = validate_chord("Ctrl+Alt+KeyC").unwrap();
		assert_eq!(display_chord(&chord), "Ctrl+Alt+C");
		let digit = validate_chord("Ctrl+Digit1").unwrap();
		assert_eq!(display_chord(&digit), "Ctrl+1");
		// Modifier order is Windows', not the order they were typed in.
		let jumbled = validate_chord("Shift+Alt+Ctrl+K").unwrap();
		assert_eq!(display_chord(&jumbled), "Ctrl+Alt+Shift+K");
	}

	#[test]
	fn a_double_tap_renders_as_the_pair_it_is_stored_as() {
		assert_eq!(
			double_tap_text(WatchedTrigger::unsided(ModifierFamily::Shift)),
			DEFAULT_CAPTURE_TRIGGER
		);
		assert_eq!(
			double_tap_text(WatchedTrigger::unsided(ModifierFamily::Control)),
			"Ctrl Ctrl"
		);
		assert_eq!(
			double_tap_text(WatchedTrigger::unsided(ModifierFamily::Alt)),
			"Alt Alt"
		);
		// The sided spellings are the family label with one letter in front, which
		// is what the settings view then expands back into "Left Ctrl".
		assert_eq!(
			double_tap_text(WatchedTrigger {
				family: ModifierFamily::Control,
				side: KeySide::Left
			}),
			"LCtrl LCtrl"
		);
		assert_eq!(
			double_tap_text(WatchedTrigger {
				family: ModifierFamily::Shift,
				side: KeySide::Right
			}),
			"RShift RShift"
		);
		assert_eq!(
			double_tap_text(WatchedTrigger {
				family: ModifierFamily::Alt,
				side: KeySide::Left
			}),
			"LAlt LAlt"
		);
	}

	#[test]
	fn no_chord_is_not_a_reachable_hotkey_id() {
		// The sentinel rests on this: `id` is `(mods.bits() << 16) | key`, and
		// `Modifiers` does not use the top bits.
		for text in ["Ctrl+Shift+Space", "Ctrl+Alt+Shift+Super+K", "F12"] {
			let chord = Shortcut::from_str(text).unwrap();
			assert_ne!(chord.id(), NO_CHORD, "{text} collides with the sentinel");
		}
	}

	/// The settings view codes against these key names, and nothing else would
	/// notice them changing — `snapshot` needs an `AppHandle`, so the shape is
	/// pinned here rather than over the real boundary.
	#[test]
	fn the_reported_state_crosses_the_boundary_in_camel_case() {
		let state = ShortcutState {
			capture: DEFAULT_CAPTURE_TRIGGER.to_owned(),
			summon: DEFAULT_SUMMON_SHORTCUT.to_owned(),
			defaults: Shortcuts::default(),
			summon_registered: false,
			summon_error: Some("Windows wouldn't accept it".to_owned()),
			capture_registered: true,
			capture_error: None,
			capture_fallback: Some(FALLBACK_CAPTURE_CHORD.to_owned()),
			summon_fallback: Some(FALLBACK_SUMMON_CHORD.to_owned()),
		};
		let payload = serde_json::to_value(&state).unwrap();

		for key in [
			"capture",
			"summon",
			"defaults",
			"summonRegistered",
			"summonError",
			"captureRegistered",
			"captureError",
			"captureFallback",
			"summonFallback",
		] {
			assert!(payload.get(key).is_some(), "get_shortcut_state is missing {key}: {payload}");
		}
		// Nine since task-020: summon gained an insurance chord of its own, because
		// a double-tap summon can be lost to a dead hook exactly as capture can.
		assert_eq!(payload.as_object().unwrap().len(), 9, "get_shortcut_state grew a field");
		assert!(!serde_json::to_string(&state).unwrap().contains('_'));
		// The defaults travel so Reset renders without a second copy of them in
		// TypeScript.
		assert_eq!(payload["defaults"]["capture"], DEFAULT_CAPTURE_TRIGGER);
		assert_eq!(payload["defaults"]["summon"], DEFAULT_SUMMON_SHORTCUT);
	}

	#[test]
	fn both_insurance_chords_are_bindable_and_distinct() {
		// They are only ever registered on a failure path, so nothing else would
		// notice if one stopped parsing — or, worse, if the two became the same
		// chord and the second registration were refused as a duplicate.
		let capture = validate_chord(FALLBACK_CAPTURE_CHORD).unwrap();
		let summon = validate_chord(FALLBACK_SUMMON_CHORD).unwrap();
		assert_ne!(capture, summon);
		// Neither may be one Windows keeps for itself, which is the failure that
		// would present as insurance that registers and then never fires.
		assert!(reserved_reason(&capture, FALLBACK_CAPTURE_CHORD).is_none());
		assert!(reserved_reason(&summon, FALLBACK_SUMMON_CHORD).is_none());
		// And neither is a shipped binding, or a fresh install with a dead hook
		// would have the insurance collide with the thing it is insuring.
		assert_ne!(
			BoundTrigger::Chord(summon),
			shipped_trigger(Role::Summon),
			"the summon insurance chord is the shipped summon chord"
		);
	}
}
