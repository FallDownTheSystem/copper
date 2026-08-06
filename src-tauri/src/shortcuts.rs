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

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{
	Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutEvent, ShortcutState as KeyState,
};

use crate::capture::{self, ModifierFamily};
use crate::store::settings::{SettingsPatch, Shortcuts};
use crate::{diagnostics, panel, store, tray, ShellError};

/// The shipped defaults. Asserted equal to task-003's `Settings::default()` in a
/// unit test — two hardcoded copies of one default is exactly the drift that
/// catches.
pub const DEFAULT_SUMMON_SHORTCUT: &str = "Ctrl+Shift+Space";
pub const DEFAULT_CAPTURE_TRIGGER: &str = "Shift Shift";

/// Registered only while the `WH_KEYBOARD_LL` hook is *not* installed.
///
/// The insurance the spec asks for: a hook that failed to attach takes the
/// double-tap trigger with it, and without this the user has no way to capture at
/// all until they open the settings view and rebind. Deliberately obscure, since
/// it exists to be available rather than to be convenient.
const FALLBACK_CAPTURE_CHORD: &str = "Ctrl+Alt+Shift+C";

/// How long a recording lease may stay open before Rust takes it back.
///
/// The lease unregisters the live chords, so an abandoned one costs the user
/// their summon shortcut until the app restarts. Generous enough that a person
/// deciding what to bind is never interrupted.
const LEASE_WATCHDOG: Duration = Duration::from_secs(120);

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
static CANONICAL_SUMMON: AtomicU32 = AtomicU32::new(NO_CHORD);
static CANONICAL_CAPTURE: AtomicU32 = AtomicU32::new(NO_CHORD);

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

/// What the capture trigger is bound to.
///
/// R-Q52 made these alternatives rather than a single shape: a double-tap is
/// recognised by task-005's keyboard hook, a conventional chord by the
/// global-shortcut plugin, and exactly one of the two is live at a time.
#[derive(Clone, Debug)]
enum CaptureBinding {
	/// The family itself is not held here: `capture::watch` owns the live value,
	/// and a second copy would be a second thing to keep in step. The text is what
	/// is persisted and shown, and it round-trips back to a family through
	/// [`validate_capture_trigger`].
	DoubleTap { text: String },
	Chord(Binding),
}

impl CaptureBinding {
	fn text(&self) -> &str {
		match self {
			Self::DoubleTap { text } => text,
			Self::Chord(binding) => &binding.text,
		}
	}
}

/// What [`begin_recording`] suspended, so the same set can be put back.
struct Lease {
	token: u64,
	summon: bool,
	capture: bool,
	fallback: bool,
}

struct Registry {
	summon: Binding,
	capture: CaptureBinding,
	/// Live only while the keyboard hook is down.
	fallback: Option<Shortcut>,
	/// Registrations `unregister` refused to retire.
	///
	/// Not merely loggable: a chord that would not retire still summons the panel,
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
			summon: Binding {
				text: DEFAULT_SUMMON_SHORTCUT.to_owned(),
				chord: Shortcut::new(
					Some(Modifiers::CONTROL | Modifiers::SHIFT),
					Code::Space,
				),
				registered: false,
				error: None,
			},
			capture: CaptureBinding::DoubleTap {
				text: DEFAULT_CAPTURE_TRIGGER.to_owned(),
			},
			fallback: None,
			stale: Vec::new(),
			lease: None,
		}
	}
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

/// A validated capture trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureTrigger {
	DoubleTap(ModifierFamily),
	Chord(Shortcut),
}

/// The parser's own modifier tokens, so "is this modifiers only?" is decided by
/// the same vocabulary the parser uses rather than by a second list that can
/// drift from it.
fn is_modifier_token(token: &str) -> bool {
	matches!(
		token.trim().to_ascii_uppercase().as_str(),
		"ALT"
			| "OPTION" | "CONTROL"
			| "CTRL" | "COMMAND"
			| "CMD" | "SUPER"
			| "SHIFT" | "COMMANDORCONTROL"
			| "COMMANDORCTRL"
			| "CMDORCTRL" | "CMDORCONTROL"
	)
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
pub fn validate_summon_chord(text: &str) -> Result<Shortcut, ShellError> {
	let text = text.trim();
	if text.is_empty() {
		return Err(ShellError::InvalidChord(
			"Press the keys you want to use.".to_owned(),
		));
	}

	let tokens: Vec<&str> = text.split('+').collect();
	if tokens.iter().all(|token| is_modifier_token(token)) {
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

	match reserved_reason(&chord, text) {
		Some(why) => Err(ShellError::Reserved(why)),
		None => Ok(chord),
	}
}

/// The double-tap families Copper offers.
///
/// `Win Win` is deliberately absent: double-tapping it fights the Start menu,
/// which opens on the *release* of a bare Windows key.
fn double_tap_family(token: &str) -> Option<ModifierFamily> {
	match token.to_ascii_uppercase().as_str() {
		"SHIFT" => Some(ModifierFamily::Shift),
		"CTRL" | "CONTROL" => Some(ModifierFamily::Control),
		"ALT" => Some(ModifierFamily::Alt),
		_ => None,
	}
}

/// Accepts both shapes R-Q52 allows: a bare-modifier double-tap written as
/// `"<Modifier> <Modifier>"`, or any conventional chord.
pub fn validate_capture_trigger(text: &str) -> Result<CaptureTrigger, ShellError> {
	let text = text.trim();
	let tokens: Vec<&str> = text.split_whitespace().collect();

	if tokens.len() == 2 && tokens[0].eq_ignore_ascii_case(tokens[1]) {
		if let Some(family) = double_tap_family(tokens[0]) {
			return Ok(CaptureTrigger::DoubleTap(family));
		}
		if matches!(tokens[0].to_ascii_uppercase().as_str(), "WIN" | "SUPER" | "COMMAND" | "CMD") {
			return Err(ShellError::Reserved(
				"Double-tapping the Windows key opens the Start menu, so Copper can't use it."
					.to_owned(),
			));
		}
		return Err(ShellError::InvalidChord(format!(
			"{text} isn't a double-tap Copper recognises. Use Shift, Ctrl or Alt."
		)));
	}

	validate_summon_chord(text).map(CaptureTrigger::Chord)
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

fn double_tap_text(family: ModifierFamily) -> String {
	let label = family_label(family);
	format!("{label} {label}")
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
	panel::summon_or_log(app);
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

/// Which handler a registration carries. The two are never interchangeable — one
/// reveals a window, the other reads the foreground selection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
	Summon,
	Capture,
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
		"Windows wouldn't accept {text} — another app is probably using it. Choose a different \
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
	summon_registered: bool,
	summon_error: Option<String>,
	/// A double-tap is live whenever the keyboard hook is; a conventional chord is
	/// live when the OS accepted it.
	capture_registered: bool,
	capture_error: Option<String>,
	/// Present only while the keyboard hook is down and the insurance chord is
	/// standing in for the double-tap.
	capture_fallback: Option<String>,
}

fn snapshot(app: &AppHandle, registry: &Registry) -> ShortcutState {
	let (capture_registered, capture_error) = match &registry.capture {
		// Nothing to register: the hook either recognises the double-tap or it is
		// not installed, and the fallback chord below is what covers the latter.
		CaptureBinding::DoubleTap { .. } => (capture::hook_installed(app), None),
		CaptureBinding::Chord(binding) => (binding.registered, binding.error.clone()),
	};

	ShortcutState {
		capture: registry.capture.text().to_owned(),
		summon: registry.summon.text.clone(),
		defaults: Shortcuts {
			capture: DEFAULT_CAPTURE_TRIGGER.to_owned(),
			summon: DEFAULT_SUMMON_SHORTCUT.to_owned(),
		},
		summon_registered: registry.summon.registered,
		summon_error: registry.summon.error.clone(),
		capture_registered,
		capture_error,
		capture_fallback: registry.fallback.as_ref().map(display_chord),
	}
}

// --- persistence -------------------------------------------------------------

/// Writes both chords, since they share one `Shortcuts` struct.
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

// --- startup -----------------------------------------------------------------

/// Points the keyboard hook at the persisted trigger, before capture starts.
///
/// Separate from [`install`] and called earlier for one reason: the first
/// double-tap after launch must be judged against the user's binding rather than
/// the compiled-in default, and `capture::start_capture` installs the hook.
pub fn prepare_capture(app: &AppHandle) {
	let stored = store::commands::settings(app).shortcuts;
	let mut registry = registry();

	let trigger = match validate_capture_trigger(&stored.capture) {
		Ok(trigger) => trigger,
		Err(err) => {
			diagnostics::log_error(&format!(
				"[copper] shortcuts: the stored capture trigger {:?} is not usable ({}); falling \
				 back to {DEFAULT_CAPTURE_TRIGGER}",
				stored.capture,
				err.message()
			));
			CaptureTrigger::DoubleTap(ModifierFamily::Shift)
		}
	};

	apply_capture_locally(&mut registry, trigger);
}

/// Puts a validated trigger into the registry and the hook's atomic. Cannot fail
/// — which is why `set_capture_trigger` may persist before calling it.
fn apply_capture_locally(registry: &mut Registry, trigger: CaptureTrigger) {
	match trigger {
		CaptureTrigger::DoubleTap(family) => {
			capture::watch(family);
			registry.capture = CaptureBinding::DoubleTap {
				text: double_tap_text(family),
			};
		}
		CaptureTrigger::Chord(chord) => {
			// The two are mutually exclusive at runtime: a conventional chord means
			// the hook has no double-tap to recognise.
			capture::watch(ModifierFamily::Off);
			registry.capture = CaptureBinding::Chord(Binding {
				text: display_chord(&chord),
				chord,
				registered: false,
				error: None,
			});
		}
	}
}

/// Registers everything the OS has to know about, after capture has started.
///
/// Returns nothing and propagates nothing. A shortcut that will not register must
/// leave the app running with a working tray: for an app that starts hidden,
/// returning `Err` from `setup()` is as fatal as panicking, and the tray is the
/// recovery path this failure is reported through.
pub fn install(app: &AppHandle) {
	let stored = store::commands::settings(app).shortcuts;
	let mut registry = registry();

	let chord = validate_summon_chord(&stored.summon).unwrap_or_else(|err| {
		diagnostics::log_error(&format!(
			"[copper] shortcuts: the stored summon chord {:?} is not usable ({}); falling back to \
			 {DEFAULT_SUMMON_SHORTCUT}",
			stored.summon,
			err.message()
		));
		Registry::shipped().summon.chord
	});

	registry.summon = Binding {
		text: display_chord(&chord),
		chord,
		registered: false,
		error: None,
	};
	match register(app, chord, Role::Summon) {
		Ok(()) => {
			registry.summon.registered = true;
			CANONICAL_SUMMON.store(chord.id(), Ordering::Relaxed);
		}
		Err(err) => {
			diagnostics::log_error(&format!(
				"[copper] shortcuts: the summon chord {} could not be registered: {err}",
				registry.summon.text
			));
			registry.summon.error = Some(registration_failed_message(&registry.summon.text));
		}
	}

	if let CaptureBinding::Chord(binding) = &registry.capture {
		let chord = binding.chord;
		let text = binding.text.clone();
		match register(app, chord, Role::Capture) {
			Ok(()) => {
				CANONICAL_CAPTURE.store(chord.id(), Ordering::Relaxed);
				if let CaptureBinding::Chord(binding) = &mut registry.capture {
					binding.registered = true;
				}
			}
			Err(err) => {
				diagnostics::log_error(&format!(
					"[copper] shortcuts: the capture chord {text} could not be registered: {err}"
				));
				if let CaptureBinding::Chord(binding) = &mut registry.capture {
					binding.error = Some(registration_failed_message(&text));
				}
			}
		}
	}

	ensure_fallback(app, &mut registry);
	tray::report_summon(app, registry.summon.registered);
}

/// Registers the insurance chord when — and only when — the keyboard hook is down
/// and the capture binding is a double-tap the hook can no longer recognise.
fn ensure_fallback(app: &AppHandle, registry: &mut Registry) {
	let wanted = matches!(registry.capture, CaptureBinding::DoubleTap { .. })
		&& !capture::hook_installed(app);

	match (wanted, registry.fallback) {
		(true, None) => match Shortcut::from_str(FALLBACK_CAPTURE_CHORD) {
			Ok(chord) => match register(app, chord, Role::Capture) {
				Ok(()) => {
					CANONICAL_CAPTURE.store(chord.id(), Ordering::Relaxed);
					registry.fallback = Some(chord);
					diagnostics::log(&format!(
						"[copper] shortcuts: the keyboard hook is unavailable; capture is reachable \
						 through {FALLBACK_CAPTURE_CHORD}"
					));
				}
				Err(err) => diagnostics::log_error(&format!(
					"[copper] shortcuts: the fallback capture chord could not be registered: {err}"
				)),
			},
			Err(err) => diagnostics::log_error(&format!(
				"[copper] shortcuts: the fallback capture chord is not parseable: {err}"
			)),
		},
		(false, Some(chord)) => {
			registry.fallback = None;
			retire(app, registry, chord);
		}
		_ => {}
	}
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
	if registry.summon.registered {
		let _ = app.global_shortcut().unregister(registry.summon.chord);
	}
	if let CaptureBinding::Chord(binding) = &registry.capture {
		if binding.registered {
			let _ = app.global_shortcut().unregister(binding.chord);
		}
	}
	if let Some(chord) = registry.fallback {
		let _ = app.global_shortcut().unregister(chord);
	}
}

// --- the rebind protocol -----------------------------------------------------

/// Rebinds the summon chord, in the order the anti-lockout guarantee needs.
///
/// Register the new chord before retiring the old one, because the plugin offers
/// no atomic replace and the two orderings fail asymmetrically: this one can
/// briefly leave two chords bound, which is recoverable; the other can leave none,
/// which is the lockout the whole protocol exists to prevent. Persistence happens
/// in the *middle*, while the old chord is still live — done afterwards, a write
/// failure would leave the runtime on the new chord and the file on the old one.
pub fn set_summon(app: &AppHandle, text: &str) -> Result<ShortcutState, ShellError> {
	let chord = validate_summon_chord(text)?;
	let mut registry = registry();
	retry_stale(app, &mut registry);

	// Compared against what is *actually registered*, not against the stored
	// string. Re-submitting the live binding is an idempotent success; re-submitting
	// one that is stored but not registered — the startup-failure case — is a retry
	// and has to fall through.
	if registry.summon.chord == chord && registry.summon.registered {
		return Ok(snapshot(app, &registry));
	}

	if capture_holds(&registry, chord) {
		return Err(ShellError::Reserved(format!(
			"{} is already Copper's capture shortcut. Choose a different one.",
			display_chord(&chord)
		)));
	}

	let previous = registry.summon.clone();
	let text = display_chord(&chord);

	register(app, chord, Role::Summon).map_err(|err| {
		diagnostics::log_error(&format!("[copper] shortcuts: {text} was refused: {err}"));
		registration_failed(&text)
	})?;

	if let Err(err) = persist(app, registry.capture.text(), &text) {
		// Nothing durable happened, so nothing may stay changed. The old chord was
		// never retired and is still the one that summons the panel.
		let _ = app.global_shortcut().unregister(chord);
		return Err(err);
	}

	registry.summon = Binding {
		text,
		chord,
		registered: true,
		error: None,
	};
	CANONICAL_SUMMON.store(chord.id(), Ordering::Relaxed);

	// Only now, and only if it is a different chord: after a startup-failure retry
	// the "previous" chord *is* this one, and retiring it would unregister what was
	// just registered.
	if previous.registered && previous.chord != chord {
		retire(app, &mut registry, previous.chord);
	}

	tray::report_summon(app, true);
	Ok(snapshot(app, &registry))
}

fn capture_holds(registry: &Registry, chord: Shortcut) -> bool {
	match &registry.capture {
		CaptureBinding::Chord(binding) => binding.registered && binding.chord == chord,
		CaptureBinding::DoubleTap { .. } => registry.fallback == Some(chord),
	}
}

/// Rebinds the capture trigger.
///
/// A double-tap swap **persists first**: pointing the hook's atomic at another
/// family cannot fail, so writing first means the file and the runtime can never
/// disagree. A conventional chord has a registration that can fail, so it follows
/// the summon ordering instead.
pub fn set_capture(app: &AppHandle, text: &str) -> Result<ShortcutState, ShellError> {
	let trigger = validate_capture_trigger(text)?;
	let mut registry = registry();
	retry_stale(app, &mut registry);

	match trigger {
		CaptureTrigger::DoubleTap(family) => {
			let text = double_tap_text(family);
			if registry.capture.text() == text {
				return Ok(snapshot(app, &registry));
			}
			persist(app, &text, &registry.summon.text)?;
			let previous = registry.capture.clone();
			apply_capture_locally(&mut registry, trigger);
			if let CaptureBinding::Chord(binding) = previous {
				CANONICAL_CAPTURE.store(NO_CHORD, Ordering::Relaxed);
				if binding.registered {
					retire(app, &mut registry, binding.chord);
				}
			}
			// The hook may be down, in which case the double-tap needs the insurance
			// chord that a conventional binding did not.
			ensure_fallback(app, &mut registry);
		}
		CaptureTrigger::Chord(chord) => {
			let text = display_chord(&chord);
			if let CaptureBinding::Chord(binding) = &registry.capture {
				if binding.chord == chord && binding.registered {
					return Ok(snapshot(app, &registry));
				}
			}
			if registry.summon.registered && registry.summon.chord == chord {
				return Err(ShellError::Reserved(format!(
					"{text} is already Copper's summon shortcut. Choose a different one."
				)));
			}

			register(app, chord, Role::Capture).map_err(|err| {
				diagnostics::log_error(&format!("[copper] shortcuts: {text} was refused: {err}"));
				registration_failed(&text)
			})?;

			if let Err(err) = persist(app, &text, &registry.summon.text) {
				let _ = app.global_shortcut().unregister(chord);
				return Err(err);
			}

			let previous = registry.capture.clone();
			apply_capture_locally(&mut registry, trigger);
			if let CaptureBinding::Chord(binding) = &mut registry.capture {
				binding.registered = true;
			}
			CANONICAL_CAPTURE.store(chord.id(), Ordering::Relaxed);

			if let CaptureBinding::Chord(binding) = previous {
				if binding.registered && binding.chord != chord {
					retire(app, &mut registry, binding.chord);
				}
			}
			// The fallback exists to keep a *double-tap* reachable; a chord binding
			// does not need it and having both would claim a hotkey for nothing.
			ensure_fallback(app, &mut registry);
		}
	}

	Ok(snapshot(app, &registry))
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

	let summon = registry.summon.registered;
	let capture = matches!(&registry.capture, CaptureBinding::Chord(binding) if binding.registered);
	let fallback = registry.fallback.is_some();

	// `registered` means "live right now", so it goes false while suspended. Any
	// other reading breaks the setters: re-submitting the chord that is currently
	// bound would look unchanged, return early, and leave it unregistered when the
	// lease is released.
	if summon {
		let _ = app.global_shortcut().unregister(registry.summon.chord);
		registry.summon.registered = false;
		CANONICAL_SUMMON.store(NO_CHORD, Ordering::Relaxed);
	}
	if let CaptureBinding::Chord(binding) = &mut registry.capture {
		if binding.registered {
			let _ = app.global_shortcut().unregister(binding.chord);
			binding.registered = false;
		}
	}
	if let Some(chord) = registry.fallback {
		let _ = app.global_shortcut().unregister(chord);
	}
	if capture || fallback {
		CANONICAL_CAPTURE.store(NO_CHORD, Ordering::Relaxed);
	}

	registry.lease = Some(Lease {
		token,
		summon,
		capture,
		fallback,
	});
	LEASE_OPEN.store(true, Ordering::Relaxed);
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

	if lease.summon && replaced != Some(Role::Summon) {
		let chord = registry.summon.chord;
		match register(app, chord, Role::Summon) {
			Ok(()) => {
				registry.summon.registered = true;
				CANONICAL_SUMMON.store(chord.id(), Ordering::Relaxed);
			}
			Err(err) => {
				// The one failure a user cannot recover from without being told, so it
				// reaches both surfaces the startup failure reaches.
				registry.summon.error = Some(registration_failed_message(&registry.summon.text));
				diagnostics::log_error(&format!(
					"[copper] shortcuts: the summon chord could not be restored after recording: \
					 {err}"
				));
				tray::report_summon(app, false);
			}
		}
	}

	if lease.capture && replaced != Some(Role::Capture) {
		if let CaptureBinding::Chord(binding) = &mut registry.capture {
			let chord = binding.chord;
			match register(app, chord, Role::Capture) {
				Ok(()) => {
					binding.registered = true;
					CANONICAL_CAPTURE.store(chord.id(), Ordering::Relaxed);
				}
				Err(err) => {
					binding.error = Some(registration_failed_message(&binding.text));
					diagnostics::log_error(&format!(
						"[copper] shortcuts: the capture chord could not be restored after \
						 recording: {err}"
					));
				}
			}
		}
	}

	if lease.fallback && replaced != Some(Role::Capture) {
		if let Some(chord) = registry.fallback {
			if register(app, chord, Role::Capture).is_ok() {
				CANONICAL_CAPTURE.store(chord.id(), Ordering::Relaxed);
			}
		}
	}
}

/// Cancels whatever session is open. Idempotent, and deliberately not fussy about
/// the token: a caller asking to stop recording must never be able to leave the
/// chords suspended because it quoted a token that had already been superseded.
pub fn cancel_recording(app: &AppHandle) -> ShortcutState {
	let mut registry = registry();
	restore_lease(app, &mut registry, None);
	snapshot(app, &registry)
}

/// Applies a recorded chord to one of the two bindings.
///
/// Rejects a stale token — unlike cancel, this one *writes*, and applying a chord
/// recorded in a session the user has already left is a change they did not ask
/// for.
pub fn commit_recording(
	app: &AppHandle,
	token: u64,
	target: &str,
	chord: &str,
) -> Result<ShortcutState, ShellError> {
	let role = match target {
		"summon" => Role::Summon,
		"capture" => Role::Capture,
		other => {
			return Err(ShellError::Invalid(format!(
				"{other} is not a shortcut Copper can rebind."
			)))
		}
	};

	{
		let registry = registry();
		match &registry.lease {
			Some(lease) if lease.token == token => {}
			_ => {
				return Err(ShellError::StaleToken(
					"That recording has already finished. Try again.".to_owned(),
				))
			}
		}
	}

	// The lock is released between the check above and the setters below because
	// they take it themselves; a superseding `begin` in that window can only make
	// the token stale, which the setters' own outcome then reflects.
	let outcome = match role {
		Role::Summon => set_summon(app, chord),
		Role::Capture => set_capture(app, chord),
	};

	let mut registry = registry();
	// Whatever happened, the session is over: on success the other bindings come
	// back and the replaced one is already live; on failure everything comes back
	// exactly as it was, which is what makes a refused chord a no-op rather than a
	// lockout.
	restore_lease(app, &mut registry, outcome.is_ok().then_some(role));
	outcome.map(|_| snapshot(app, &registry))
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
pub async fn get_shortcut_state(app: AppHandle) -> Reply<ShortcutState> {
	let registry = registry();
	Ok(snapshot(&app, &registry))
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
	use crate::store::settings::Settings;

	#[test]
	fn the_defaults_match_the_ones_the_store_ships() {
		// Two hardcoded copies of one default is the drift this catches.
		let shipped = Settings::default().shortcuts;
		assert_eq!(shipped.summon, DEFAULT_SUMMON_SHORTCUT);
		assert_eq!(shipped.capture, DEFAULT_CAPTURE_TRIGGER);
	}

	#[test]
	fn the_built_default_chord_equals_the_parsed_one() {
		// `Registry::shipped` builds the chord rather than parsing it, so that no
		// release-build path can abort on an `expect`. This is where the two
		// spellings are held together.
		assert_eq!(
			Registry::shipped().summon.chord,
			Shortcut::from_str(DEFAULT_SUMMON_SHORTCUT).unwrap()
		);
	}

	#[test]
	fn valid_summon_chords_parse() {
		assert!(validate_summon_chord("Ctrl+Shift+Space").is_ok());
		// Maps to Control on Windows.
		let mapped = validate_summon_chord("CommandOrControl+K").unwrap();
		assert!(mapped.mods.contains(Modifiers::CONTROL));
		assert_eq!(mapped.key, Code::KeyK);
		assert!(validate_summon_chord("Ctrl+Alt+C").is_ok());
	}

	#[test]
	fn modifier_only_input_is_named_as_such() {
		// The parser calls this "invalid hotkey format", which is the same message
		// it gives for genuine nonsense — two different mistakes to make.
		assert_eq!(
			validate_summon_chord("Shift+Ctrl").unwrap_err().kind(),
			"modifier-only"
		);
		assert_eq!(validate_summon_chord("Shift").unwrap_err().kind(), "modifier-only");
	}

	#[test]
	fn malformed_input_is_rejected_before_the_os_sees_it() {
		// Modifiers must precede the main key.
		assert_eq!(
			validate_summon_chord("Ctrl+KeyQ+Shift").unwrap_err().kind(),
			"invalid-chord"
		);
		assert_eq!(validate_summon_chord("Nonsense").unwrap_err().kind(), "invalid-chord");
		assert_eq!(validate_summon_chord("").unwrap_err().kind(), "invalid-chord");
	}

	#[test]
	fn combinations_windows_never_delivers_are_reserved() {
		for chord in ["Super+L", "Alt+Tab", "Ctrl+Alt+Delete", "PrintScreen"] {
			let err = validate_summon_chord(chord).unwrap_err();
			assert_eq!(err.kind(), "reserved", "{chord} was not reserved");
			assert!(!err.message().is_empty());
		}
		// A Windows-key chord that also carries a combining modifier is fine —
		// Windows only keeps the bare ones for itself.
		assert!(validate_summon_chord("Ctrl+Super+K").is_ok());
	}

	#[test]
	fn capture_accepts_both_shapes_r_q52_allows() {
		assert_eq!(
			validate_capture_trigger("Shift Shift").unwrap(),
			CaptureTrigger::DoubleTap(ModifierFamily::Shift)
		);
		assert_eq!(
			validate_capture_trigger("Ctrl Ctrl").unwrap(),
			CaptureTrigger::DoubleTap(ModifierFamily::Control)
		);
		assert_eq!(
			validate_capture_trigger("Alt Alt").unwrap(),
			CaptureTrigger::DoubleTap(ModifierFamily::Alt)
		);
		// Case is not part of the binding.
		assert_eq!(
			validate_capture_trigger("control control").unwrap(),
			CaptureTrigger::DoubleTap(ModifierFamily::Control)
		);
		assert!(matches!(
			validate_capture_trigger("Ctrl+Alt+C").unwrap(),
			CaptureTrigger::Chord(_)
		));
	}

	#[test]
	fn capture_rejects_the_shapes_it_cannot_service() {
		// Double-tapping Win opens the Start menu on the release of a bare press.
		assert_eq!(validate_capture_trigger("Win Win").unwrap_err().kind(), "reserved");
		// A bare modifier is not a double-tap.
		assert_eq!(validate_capture_trigger("Shift").unwrap_err().kind(), "modifier-only");
		// Two different modifiers are neither a double-tap nor a chord.
		assert_eq!(
			validate_capture_trigger("Shift Ctrl").unwrap_err().kind(),
			"invalid-chord"
		);
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
			let chord = validate_summon_chord(text).unwrap();
			let rendered = display_chord(&chord);
			assert_eq!(
				validate_summon_chord(&rendered).unwrap(),
				chord,
				"{text} rendered as {rendered}, which does not read back"
			);
		}
	}

	#[test]
	fn the_display_spelling_is_the_one_a_person_reads() {
		let chord = validate_summon_chord("Ctrl+Alt+KeyC").unwrap();
		assert_eq!(display_chord(&chord), "Ctrl+Alt+C");
		let digit = validate_summon_chord("Ctrl+Digit1").unwrap();
		assert_eq!(display_chord(&digit), "Ctrl+1");
		// Modifier order is Windows', not the order they were typed in.
		let jumbled = validate_summon_chord("Shift+Alt+Ctrl+K").unwrap();
		assert_eq!(display_chord(&jumbled), "Ctrl+Alt+Shift+K");
	}

	#[test]
	fn a_double_tap_renders_as_the_pair_it_is_stored_as() {
		assert_eq!(double_tap_text(ModifierFamily::Shift), DEFAULT_CAPTURE_TRIGGER);
		assert_eq!(double_tap_text(ModifierFamily::Control), "Ctrl Ctrl");
		assert_eq!(double_tap_text(ModifierFamily::Alt), "Alt Alt");
		// And reads back as the same family.
		for family in [
			ModifierFamily::Shift,
			ModifierFamily::Control,
			ModifierFamily::Alt,
		] {
			assert_eq!(
				validate_capture_trigger(&double_tap_text(family)).unwrap(),
				CaptureTrigger::DoubleTap(family)
			);
		}
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
		] {
			assert!(payload.get(key).is_some(), "get_shortcut_state is missing {key}: {payload}");
		}
		assert_eq!(payload.as_object().unwrap().len(), 8, "get_shortcut_state grew a field");
		assert!(!serde_json::to_string(&state).unwrap().contains('_'));
		// The defaults travel so Reset renders without a second copy of them in
		// TypeScript.
		assert_eq!(payload["defaults"]["capture"], DEFAULT_CAPTURE_TRIGGER);
		assert_eq!(payload["defaults"]["summon"], DEFAULT_SUMMON_SHORTCUT);
	}

	#[test]
	fn the_fallback_chord_is_bindable() {
		// It is only ever registered on a failure path, so nothing else would notice
		// if it stopped parsing.
		assert!(validate_summon_chord(FALLBACK_CAPTURE_CHORD).is_ok());
	}
}
