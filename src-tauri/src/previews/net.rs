//! The one outbound request a *note* can cause, and every bound on it.
//!
//! This is the only file in the module that touches the network, which is what
//! lets everything else — the consent gate, the cache, the extraction rules — be
//! tested without one. Nothing here is unit-tested: a test of this file would
//! have to reach a real host, and a suite that does is a suite that fails on an
//! aeroplane.
//!
//! # What is *not* sent
//!
//! - **No cookies.** reqwest's `cookies` feature is off, so there is no cookie
//!   store to populate: `Set-Cookie` is ignored structurally rather than by a
//!   rule, and nothing about one fetch can be correlated with the next.
//! - **No referrer.** `referer(false)` is not the default — reqwest attaches one
//!   across redirects otherwise — and the referrer here would be the previous
//!   URL from the *user's own note*.
//! - **No identifying User-Agent.** [`USER_AGENT`] is a fixed string with no
//!   version in it. A version would change with every release and become one
//!   more bit distinguishing this install from the next.
//! - **No proxy.** reqwest's `system-proxy` feature is off, so a Windows proxy
//!   configuration is not read. Behind a corporate proxy previews simply do not
//!   resolve, which is the safe direction: the alternative is routing a note's
//!   URLs through a gateway the user did not know was involved.
//!
//! # What comes back
//!
//! Bounded twice over, because `Content-Length` is advisory and may be absent or
//! simply wrong — the same TOCTOU reasoning `attachments::read_capped` records.
//! The body is read chunk by chunk and stopped at a cap by the read itself.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::{Client, Response, Url};

use super::{cache, Page, Pages, Pending};

/// Fixed, and deliberately honest about what it is. A crawler that lies about
/// being a browser gets better coverage; it also means a site operator cannot
/// tell what is asking, which is not a trade this feature should make on the
/// user's behalf.
const USER_AGENT: &str = "Copper/1 (+https://github.com/FallDownTheSystem/copper)";

/// How long the connection alone may take.
///
/// Separate from the total below because they answer different questions: this
/// one is "is anything there", and three seconds is generous for that.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// The whole request, header to last byte.
///
/// reqwest's `timeout` is a **total** deadline covering the response body rather
/// than an idle timeout — `updater::DOWNLOAD_TIMEOUT` records the same fact
/// about the same client — so this is not "how long a stall is tolerated" but
/// "how long the entire fetch may take". Eight seconds is far more than a
/// `<head>` needs and short enough that a tarpit is dropped while the user is
/// still looking at the note.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(8);

/// How much of a page is read.
///
/// Everything this module wants is in `<head>`, which is in the first few
/// kilobytes of every document ever served. Half a megabyte is the point past
/// which reading more cannot help — so the body is **truncated** here rather
/// than refused, and a page with a megabyte of inline script after its metadata
/// still previews.
const MAX_HTML_BYTES: usize = 512 * 1024;

/// How many redirects are followed before the chain is abandoned.
const MAX_REDIRECTS: usize = 5;

/// The production [`Pages`]. One per process, built lazily.
pub struct Web;

/// Built once and shared: a client owns a connection pool, and a fresh one per
/// preview would open a new TLS session for every link in a note.
static CLIENT: OnceLock<Option<Client>> = OnceLock::new();

fn client() -> Option<&'static Client> {
	CLIENT
		.get_or_init(|| {
			// reqwest is compiled with `rustls-no-provider`, which means it ships no
			// crypto provider and **panics** inside `build()` when the process has no
			// default installed. `tauri-plugin-updater` installs `ring` before its own
			// first request, so without this line the first preview fetched by a
			// session that never checked for updates would take the process down.
			// Installing twice is a no-op and returns an error, which is why the
			// result is discarded rather than checked.
			if rustls::crypto::CryptoProvider::get_default().is_none() {
				let _ = rustls::crypto::ring::default_provider().install_default();
			}

			Client::builder()
				.user_agent(USER_AGENT)
				.connect_timeout(CONNECT_TIMEOUT)
				.timeout(TOTAL_TIMEOUT)
				.referer(false)
				.redirect(policy())
				.build()
				.map_err(|err| {
					crate::diagnostics::log_error(&format!(
						"[copper] previews: no HTTP client could be built, so no link preview will \
						 load this session: {err}"
					));
					err
				})
				.ok()
		})
		.as_ref()
}

/// Follows up to [`MAX_REDIRECTS`] hops, **re-vetting the host at every one**.
///
/// The initial check in `previews::vet` bounds only the URL the note wrote
/// down. Without this, a link to a public shortener that answers `302
/// http://192.168.1.1/` — or to `169.254.169.254` — would reach exactly the
/// hosts that check exists to keep this process away from, and the note would
/// have been written by whoever sent the `.copper` file.
fn policy() -> reqwest::redirect::Policy {
	reqwest::redirect::Policy::custom(|attempt| {
		if attempt.previous().len() >= MAX_REDIRECTS {
			return attempt.stop();
		}
		let url = attempt.url().clone();
		if !matches!(url.scheme(), "http" | "https") || !super::is_public_host(&url) {
			return attempt.stop();
		}
		attempt.follow()
	})
}

impl Pages for Web {
	fn page<'a>(&'a self, url: &'a Url) -> Pending<'a, Option<Page>> {
		Box::pin(fetch_page(url))
	}

	fn image<'a>(&'a self, url: &'a Url) -> Pending<'a, Option<Vec<u8>>> {
		Box::pin(fetch_image(url))
	}
}

/// Fetches and decodes a page, or nothing. Every failure is silent per AC-6.
async fn fetch_page(url: &Url) -> Option<Page> {
	let response = get(url).await?;

	// The type is checked before a byte of the body is read. A `.zip` behind a
	// link would otherwise be downloaded to the cap and handed to an HTML parser,
	// which is half a megabyte of transfer and a parse for a certain miss.
	let content_type = response
		.headers()
		.get(reqwest::header::CONTENT_TYPE)
		.and_then(|value| value.to_str().ok())
		.unwrap_or_default()
		.to_ascii_lowercase();
	let kind = content_type.split(';').next().unwrap_or_default().trim();
	if !matches!(kind, "text/html" | "application/xhtml+xml") {
		return None;
	}

	// After the redirects, which is what a relative `og:image` resolves against.
	let final_url = response.url().clone();
	let bytes = read_capped(response, MAX_HTML_BYTES, Truncate::Allowed).await?;

	// Lossy, and the documented limitation of this feature: reqwest's `charset`
	// feature is off — it would add `encoding_rs`, a large table crate — so a
	// windows-1252 or Shift_JIS page yields mojibake in its title rather than the
	// text. UTF-8 is what the overwhelming majority of the web declares, and a
	// wrong-looking title is a much smaller cost than a second decoder.
	Some(Page {
		html: String::from_utf8_lossy(&bytes).into_owned(),
		url: final_url,
	})
}

/// Fetches an `og:image`. The bytes are not inspected here — `cache::write_image`
/// sniffs them and applies the decoder's own ceilings.
async fn fetch_image(url: &Url) -> Option<Vec<u8>> {
	let response = get(url).await?;
	// Refused rather than truncated, unlike a page: half an image is not a smaller
	// image, it is a decode failure, and reading four megabytes to discover that
	// is worse than stopping.
	read_capped(response, cache::MAX_IMAGE_BYTES, Truncate::Refused).await
}

async fn get(url: &Url) -> Option<Response> {
	let response = client()?.get(url.clone()).send().await.ok()?;
	response.status().is_success().then_some(response)
}

/// Whether running past the cap yields what was read so far, or nothing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Truncate {
	Allowed,
	Refused,
}

/// Reads at most `limit` bytes, chunk by chunk.
///
/// The bound is applied by the read itself rather than by a `Content-Length`
/// check, which is advisory, absent from a chunked response, and free to lie —
/// the same reasoning `attachments::read_take` carries about a file that can
/// grow between the `stat` and the read.
async fn read_capped(mut response: Response, limit: usize, over: Truncate) -> Option<Vec<u8>> {
	let mut bytes: Vec<u8> = Vec::new();
	while let Some(chunk) = response.chunk().await.ok()? {
		if bytes.len() + chunk.len() > limit {
			if over == Truncate::Refused {
				return None;
			}
			bytes.extend_from_slice(&chunk[..limit - bytes.len()]);
			break;
		}
		bytes.extend_from_slice(&chunk);
	}
	(!bytes.is_empty()).then_some(bytes)
}
