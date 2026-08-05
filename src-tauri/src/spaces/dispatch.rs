//! One queue for both ways a `.copper` file reaches a running app: the argv
//! `setup()` reads on a cold launch, and the argv the single-instance plugin
//! forwards from a second process.
//!
//! # Why a dispatcher rather than doing the work inline
//!
//! The single-instance callback runs **on the main thread** — the plugin's
//! receiving window is created there and Win32 delivers messages on the creating
//! thread — so anything slow inside it stalls the UI message loop. Submission
//! therefore returns immediately and a worker does the work.
//!
//! # Every file is applied; only the presentation coalesces
//!
//! The registered shell command uses a single `"%1"` rather than `%*`, so
//! selecting three files in Explorer launches three processes and produces three
//! callbacks, in race order rather than selection order. Each carries a path the
//! user explicitly asked to open, so each is opened and each lands in `recents`;
//! the last applied is active. What collapses is the reveal and the frontend
//! refresh — one per burst, not one per file.
//!
//! Collapsing the burst to "the last request wins" was considered and rejected:
//! callback arrival order is race order, so "last" is nondeterministic, and every
//! discarded path was an explicit user open that would then never appear in
//! `recents`. It is also indistinguishable in here from two genuinely separate
//! double-clicks a moment apart, which must both be honoured.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// How long the burst window stays open after the last request. Long enough to
/// absorb an Explorer multi-select, short enough that a deliberate second
/// double-click still feels immediate.
const COALESCE_WINDOW: Duration = Duration::from_millis(300);

/// What the dispatcher does with a request once it is its turn.
///
/// A trait so the queueing, the ordering and the coalescing can be tested with
/// counters and a fake clock, which Explorer timing can neither reproduce
/// reliably nor attribute when it fails.
pub trait LaunchHost: Send + Sync + 'static {
	fn open(&self, path: &Path);
	/// Reveal the panel. Called once per burst, never once per file.
	fn present(&self);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
	pub path: Option<PathBuf>,
	/// Whether this request wants the panel revealed.
	///
	/// A **cold** launch with no path must not reveal: the panel is created
	/// hidden and stays hidden until the tray or the summon chord asks for it, so
	/// revealing on an ordinary start would pop the app open at every autostart.
	/// A **forwarded** launch always reveals — the user clearly asked for the
	/// running app. And a request carrying no path is a bare reveal that must
	/// never supersede or cancel a pending file-open.
	pub reveal: bool,
}

impl Request {
	pub fn cold(path: Option<PathBuf>) -> Self {
		Self {
			reveal: path.is_some(),
			path,
		}
	}

	pub fn forwarded(path: Option<PathBuf>) -> Self {
		Self { path, reveal: true }
	}
}

#[derive(Default)]
struct State {
	queue: VecDeque<Request>,
	host: Option<Arc<dyn LaunchHost>>,
	shutdown: bool,
}

struct Shared {
	state: Mutex<State>,
	work: Condvar,
	window: Duration,
}

pub struct Dispatcher {
	shared: Arc<Shared>,
}

impl Dispatcher {
	pub fn new(window: Duration) -> Self {
		Self {
			shared: Arc::new(Shared {
				state: Mutex::new(State::default()),
				work: Condvar::new(),
				window,
			}),
		}
	}

	/// Queues a request. Never blocks, and never drops: a request submitted
	/// before the host exists waits for it rather than being executed against a
	/// half-built app or discarded.
	pub fn submit(&self, request: Request) {
		let mut state = lock(&self.shared.state);
		state.queue.push_back(request);
		drop(state);
		self.shared.work.notify_all();
	}

	/// Opens the readiness gate and starts the worker. Everything queued so far
	/// drains in arrival order.
	pub fn start(&self, host: Arc<dyn LaunchHost>) {
		let mut state = lock(&self.shared.state);
		if state.host.is_some() {
			return;
		}
		state.host = Some(host);
		drop(state);

		let worker = Arc::clone(&self.shared);
		std::thread::spawn(move || run_worker(&worker));
		self.shared.work.notify_all();
	}
}

impl Drop for Dispatcher {
	fn drop(&mut self) {
		let mut state = lock(&self.shared.state);
		state.shutdown = true;
		drop(state);
		self.shared.work.notify_all();
	}
}

/// FIFO and serial: each request is applied to completion before the next
/// starts, which is what makes "every file reaches `recents`" true without a
/// batch API on the store, and what stops two requests interleaving their
/// settings reads and writes.
///
/// **The reveal is on the leading edge of the burst, not the trailing one.** A
/// lone double-click is the overwhelmingly common case, and waiting out the
/// coalescing window before showing the window would put the whole 300 ms on the
/// path the user actually feels. Revealing on the first reveal-bearing request
/// and suppressing the rest until the burst closes gives the same "once per
/// burst" guarantee with none of the latency.
fn run_worker(shared: &Arc<Shared>) {
	loop {
		let (host, first) = {
			let mut state = lock(&shared.state);
			loop {
				if state.shutdown {
					return;
				}
				match (state.host.clone(), state.queue.pop_front()) {
					(Some(host), Some(request)) => break (host, request),
					_ => state = shared.work.wait(state).unwrap_or_else(|err| err.into_inner()),
				}
			}
		};

		let mut revealed = false;
		let mut present_once = |request: Request| {
			if apply(host.as_ref(), request) && !revealed {
				host.present();
				revealed = true;
			}
		};
		present_once(first);

		// The burst is defined by silence, and only a genuine timeout counts as
		// silence: `wait_timeout` also returns on a notify and on a spurious wake,
		// and treating either as "the window closed" would end the burst on the very
		// event that should have extended it.
		loop {
			let next = {
				let mut state = lock(&shared.state);
				loop {
					if let Some(request) = state.queue.pop_front() {
						break Some(request);
					}
					if state.shutdown {
						break None;
					}
					let (guard, timeout) = shared
						.work
						.wait_timeout(state, shared.window)
						.unwrap_or_else(|err| err.into_inner());
					state = guard;
					if timeout.timed_out() && state.queue.is_empty() {
						break None;
					}
				}
			};
			let Some(request) = next else { break };
			present_once(request);
		}
	}
}

fn apply(host: &dyn LaunchHost, request: Request) -> bool {
	if let Some(path) = &request.path {
		host.open(path);
	}
	request.reveal
}

// --- the process-wide instance -------------------------------------------------

/// Reachable without managed state, because the single-instance callback can in
/// principle fire before `app.manage` has run — and a request that arrives then
/// must be queued, not dropped. `Manager::state` would panic, and `try_state`
/// would silently lose the file the user double-clicked.
static DISPATCHER: OnceLock<Dispatcher> = OnceLock::new();

fn shared() -> &'static Dispatcher {
	DISPATCHER.get_or_init(|| Dispatcher::new(COALESCE_WINDOW))
}

pub fn submit(request: Request) {
	shared().submit(request);
}

pub fn start(host: Arc<dyn LaunchHost>) {
	shared().start(host);
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
	mutex.lock().unwrap_or_else(|err| err.into_inner())
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::atomic::{AtomicUsize, Ordering};

	#[derive(Default)]
	struct Counting {
		opens: Mutex<Vec<PathBuf>>,
		presents: AtomicUsize,
	}

	impl LaunchHost for Counting {
		fn open(&self, path: &Path) {
			lock(&self.opens).push(path.to_path_buf());
		}
		fn present(&self) {
			self.presents.fetch_add(1, Ordering::SeqCst);
		}
	}

	impl Counting {
		fn opened(&self) -> Vec<PathBuf> {
			lock(&self.opens).clone()
		}
		fn presents(&self) -> usize {
			self.presents.load(Ordering::SeqCst)
		}
	}

	fn file(name: &str) -> Option<PathBuf> {
		Some(PathBuf::from(format!(r"D:\notes\{name}.copper")))
	}

	/// Waits for a condition rather than sleeping a guessed amount: the worker is
	/// a real thread and the window is real time, so a fixed sleep would be either
	/// slow or flaky.
	fn until(condition: impl Fn() -> bool) {
		for _ in 0..400 {
			if condition() {
				return;
			}
			std::thread::sleep(Duration::from_millis(5));
		}
		panic!("the dispatcher did not reach the expected state");
	}

	/// Waits out a whole burst window plus slack, so "no further reveal happened"
	/// is a settled fact rather than a race with the worker.
	fn settle(window: Duration) {
		std::thread::sleep(window * 4);
	}

	/// A24. Three files, three opens, **one** reveal.
	#[test]
	fn a_burst_applies_every_file_and_presents_once() {
		let host = Arc::new(Counting::default());
		let window = Duration::from_millis(40);
		let dispatcher = Dispatcher::new(window);
		dispatcher.start(Arc::clone(&host) as Arc<dyn LaunchHost>);

		for name in ["a", "b", "c"] {
			dispatcher.submit(Request::forwarded(file(name)));
		}

		until(|| host.opened().len() == 3);
		settle(window);
		assert_eq!(host.opened().len(), 3, "a file the user asked for was dropped");
		assert_eq!(host.presents(), 1, "the burst revealed more than once");
	}

	/// The reveal lands on the first reveal-bearing request rather than after the
	/// window: a lone double-click is the common case and must not pay the
	/// coalescing delay.
	#[test]
	fn the_reveal_happens_before_the_burst_window_closes() {
		let host = Arc::new(Counting::default());
		// Long enough that a trailing-edge reveal could not possibly pass this.
		let window = Duration::from_millis(400);
		let dispatcher = Dispatcher::new(window);
		dispatcher.start(Arc::clone(&host) as Arc<dyn LaunchHost>);

		let started = std::time::Instant::now();
		dispatcher.submit(Request::forwarded(file("a")));
		until(|| host.presents() == 1);

		assert!(
			started.elapsed() < window,
			"the reveal waited for the coalescing window: {:?}",
			started.elapsed()
		);
	}

	/// Arrival order is the applied order, so the last one applied is the active
	/// space and `settings.json` is internally consistent with it.
	#[test]
	fn requests_are_applied_in_arrival_order() {
		let host = Arc::new(Counting::default());
		let dispatcher = Dispatcher::new(Duration::from_millis(40));
		dispatcher.start(Arc::clone(&host) as Arc<dyn LaunchHost>);

		for name in ["first", "second", "third"] {
			dispatcher.submit(Request::forwarded(file(name)));
		}

		until(|| host.opened().len() == 3);
		let opened = host.opened();
		assert_eq!(opened[0], file("first").unwrap());
		assert_eq!(opened[2], file("third").unwrap());
	}

	/// A15 / the no-path rule: a bare reveal interleaved into a burst must not
	/// cancel or supersede a pending file-open.
	#[test]
	fn a_bare_reveal_does_not_cancel_a_pending_open() {
		let host = Arc::new(Counting::default());
		let window = Duration::from_millis(40);
		let dispatcher = Dispatcher::new(window);
		dispatcher.start(Arc::clone(&host) as Arc<dyn LaunchHost>);

		dispatcher.submit(Request::forwarded(file("a")));
		dispatcher.submit(Request::forwarded(None));
		dispatcher.submit(Request::forwarded(file("b")));

		until(|| host.opened().len() == 2);
		settle(window);
		assert_eq!(host.opened().len(), 2);
		assert_eq!(host.presents(), 1);
	}

	/// A24b. Requests submitted before the gate opens are queued, not dropped and
	/// not raced, and cold argv is always applied first.
	#[test]
	fn requests_submitted_before_readiness_drain_in_order() {
		let host = Arc::new(Counting::default());
		let dispatcher = Dispatcher::new(Duration::from_millis(40));

		dispatcher.submit(Request::cold(file("cold")));
		dispatcher.submit(Request::forwarded(file("forwarded")));
		assert!(host.opened().is_empty(), "work ran before the gate opened");

		dispatcher.start(Arc::clone(&host) as Arc<dyn LaunchHost>);

		until(|| host.opened().len() == 2);
		let opened = host.opened();
		assert_eq!(opened[0], file("cold").unwrap());
		assert_eq!(opened[1], file("forwarded").unwrap());
	}

	/// The design's create-hidden rule: an ordinary launch with no file must not
	/// pop the panel open, including at autostart.
	#[test]
	fn a_cold_launch_with_no_file_does_not_reveal() {
		let host = Arc::new(Counting::default());
		let dispatcher = Dispatcher::new(Duration::from_millis(20));
		dispatcher.start(Arc::clone(&host) as Arc<dyn LaunchHost>);

		dispatcher.submit(Request::cold(None));
		// A forwarded no-path request afterwards proves the worker is alive and the
		// absence above is the rule rather than a stalled thread.
		std::thread::sleep(Duration::from_millis(80));
		assert_eq!(host.presents(), 0);

		dispatcher.submit(Request::forwarded(None));
		until(|| host.presents() == 1);
	}

	#[test]
	fn a_cold_launch_with_a_file_reveals() {
		assert!(Request::cold(file("a")).reveal);
		assert!(!Request::cold(None).reveal);
		assert!(Request::forwarded(None).reveal);
	}

	/// Two double-clicks a moment apart are two bursts, not one.
	#[test]
	fn a_request_after_the_window_closes_presents_again() {
		let host = Arc::new(Counting::default());
		let window = Duration::from_millis(20);
		let dispatcher = Dispatcher::new(window);
		dispatcher.start(Arc::clone(&host) as Arc<dyn LaunchHost>);

		dispatcher.submit(Request::forwarded(file("a")));
		until(|| host.presents() == 1);
		// Past the window, so the second request genuinely starts a new burst
		// rather than joining the first and being suppressed by it.
		settle(window);

		dispatcher.submit(Request::forwarded(file("b")));
		until(|| host.presents() == 2);
		assert_eq!(host.opened().len(), 2);
	}
}
