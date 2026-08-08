//! The two commands the panel reaches link previews through.
//!
//! They follow the conventions the attachment commands already hold: every
//! parameter name is a single word, so Tauri's snake↔camel conversion stays a
//! no-op; neither needs a capability entry, because app-defined commands are
//! allowed for every window by default; and the picture travels as **bytes over
//! IPC** rather than as a URL the WebView fetches, which is the whole point —
//! `useMarkdown` refuses to emit an `<img>` for a Markdown image precisely so a
//! note cannot issue a read receipt, and a card that hotlinked `og:image` would
//! reopen that.
//!
//! **Both of them read the toggle store-side and answer with nothing when it is
//! off.** Not because the frontend is expected to call them anyway, but so that
//! it does not matter whether it does: a consent gate that lives only in the
//! WebView is one stale value away from a disclosure that cannot be recalled.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};

use crate::attachments::read_capped;
use crate::store::error::StoreError;
use crate::store::{self, SharedStore};

use super::{cache, net, LinkPreview};

type Reply<T> = std::result::Result<T, StoreError>;

/// A cached thumbnail is small by construction — `THUMB_MAX_EDGE` square, PNG —
/// so this is a sanity bound on a file that should never approach it, not a
/// policy. It exists because the cache directory is on disk and disk contents
/// are not a thing this process wrote in every possible world.
const MAX_CACHED_IMAGE_BYTES: u64 = 2 * 1024 * 1024;

/// Whether previews may be fetched or shown at all, plus where the cache lives.
///
/// One function, because the two questions have to be answered together: a
/// command that resolved the directory without re-reading the flag would be a
/// second place for the consent rule to be forgotten.
fn consent(app: &AppHandle, state: &SharedStore) -> Option<PathBuf> {
	if !store::lock(state).settings().link_previews {
		return None;
	}
	let config = app.path().app_config_dir().ok()?;
	Some(cache::dir(&config))
}

/// The card for one link, or `null`.
///
/// **`null` is the ordinary answer and never an error.** A page that is
/// unreachable, that timed out, that answered with a PDF, that carries no
/// metadata, or that the user has previews switched off for are all the same
/// outcome from the panel's point of view — this link has no card — and AC-6
/// requires every one of them to be silent. An error here would put a message on
/// a note over a third party's server being slow.
///
/// `url` is the href markdown-it emitted, which the frontend took from the token
/// stream: the preview is fetched for exactly the links the reader can see, and
/// for no others.
#[tauri::command]
pub async fn link_preview(
	url: String,
	app: AppHandle,
	state: State<'_, SharedStore>,
) -> Reply<Option<LinkPreview>> {
	// The lock is taken and dropped here, before the request: holding the store
	// across a network fetch would stall every capture for as long as a remote
	// host felt like taking.
	let Some(dir) = consent(&app, &state) else {
		return Ok(None);
	};

	Ok(super::preview(&dir, true, &url, &net::Web).await)
}

/// The bytes of a cached preview picture, or **an empty response**.
///
/// Empty rather than an error for the same reason `attachment_thumb` is: the
/// caller is a card that is already on screen, and "the picture is gone" is not
/// something it can usefully report — it simply renders without one. The bytes
/// go back through `tauri::ipc::Response`, which sends them raw; a `Vec<u8>`
/// return would be serialised as a JSON array of numbers.
///
/// `file` is the name the matching [`link_preview`] handed out, and it is
/// rebuilt into a path through [`cache::resolve`] rather than trusted —
/// the one door into the cache directory, exactly as `attachments::resolve` is
/// into a space's sidecar.
#[tauri::command]
pub async fn preview_image(
	file: String,
	app: AppHandle,
	state: State<'_, SharedStore>,
) -> Reply<tauri::ipc::Response> {
	let empty = || Ok(tauri::ipc::Response::new(Vec::new()));
	let Some(dir) = consent(&app, &state) else {
		return empty();
	};
	let Some(path) = cache::resolve(&dir, &file) else {
		return empty();
	};

	tauri::async_runtime::spawn_blocking(move || {
		match read_capped(&path, MAX_CACHED_IMAGE_BYTES, &file) {
			Ok(bytes) => Ok(tauri::ipc::Response::new(bytes)),
			// A missing or oversized cache file is a swept entry or a directory
			// somebody edited, and neither is news.
			Err(_) => Ok(tauri::ipc::Response::new(Vec::new())),
		}
	})
	.await
	.map_err(|err| StoreError::Io(format!("the preview image could not be read: {err}")))?
}

/// Deletes expired and surplus cache entries, once, on a thread of its own.
///
/// **Startup only, and detached.** Nothing needs it finished before the panel
/// exists, and a mid-session pass would delete an entry a card on screen is
/// about to ask for — turning a rendered preview back into a fetch, which is the
/// one thing this feature exists to avoid doing twice.
///
/// It runs whatever the toggle says. The cache is not deleted when previews are
/// switched off — that would make off-then-on re-fetch and disclose a second
/// time — but an install that turned the feature off a year ago should still not
/// be holding a year-old directory, and expiry is not a disclosure.
pub fn start_prune(app: &AppHandle) {
	let Ok(config) = app.path().app_config_dir() else {
		return;
	};
	std::thread::spawn(move || cache::prune(&cache::dir(&config)));
}
