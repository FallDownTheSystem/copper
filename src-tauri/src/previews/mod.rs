//! Link previews: the only code in Copper that fetches a URL a *note* named.
//!
//! # What enabling this actually does
//!
//! Every other feature in this app is local. Turning link previews on makes
//! Copper issue an HTTP GET to whatever third-party host a note happens to
//! mention, which discloses to whoever operates that host: the URL, the reader's
//! IP address, the moment the note was read, and a User-Agent naming Copper.
//! Note bodies are pasted from anywhere — an internal ticket, an unlisted share
//! link, a document nobody else is supposed to know exists — so a fetch can
//! confirm to a stranger that a private URL exists and is being looked at.
//!
//! That is the same disclosure `useMarkdown`'s image rule refuses for Markdown
//! images, so the two have to agree, and they do: nothing here ever hands the
//! WebView a remote URL. An `og:image` is downloaded *by this module*,
//! downscaled through [`crate::attachments::thumb`], and delivered as bytes over
//! IPC. A note body still cannot issue an outbound request of its own.
//!
//! # Where the consent lives
//!
//! **In `settings.json`, read store-side, and read again at every step.**
//! `commands::consent` is the gate: it reads `settings.link_previews` through
//! the store lock and hands [`preview`] a closure that re-reads it, so "no
//! fetches when the toggle is off" is a property of the code rather than a rule
//! the frontend has to keep. This is `Settings::insertion()`'s arrangement —
//! read on the store side rather than taken as a command parameter — applied to
//! a decision where a frontend-only gate would be one stale `settings.value`
//! away from a leak that cannot be taken back.
//!
//! The re-reads are not belt and braces. A fetch takes seconds; the switch is in
//! a panel the user can be looking at while it runs. Consent is checked before
//! the page request, again before the `og:image` request to whatever second host
//! the page names, and again before the cache is written — so withdrawing it
//! stops the next disclosure rather than only the next *link*. The frontend's
//! own epoch drops a late response, but by then the request has already been
//! made, which is why that is not the gate either.
//!
//! Turning the toggle **off does not delete the cache**. Deleting it would mean
//! off-then-on re-fetches every URL and leaks a second time, which is the
//! opposite of what switching it off is for.
//!
//! # What is bounded, and where
//!
//! - The URL, by [`vet`]: `http(s)` only, no credentials, no loopback or
//!   private-range host — applied again to every redirect hop in [`net`].
//! - The page, by `MAX_HTML_BYTES` and a `Content-Type` check, both in [`net`].
//! - The image, by `MAX_IMAGE_BYTES` and then by `thumb::thumbnail`, which
//!   carries the decompression-bomb ceilings the attachment path already
//!   applies.
//! - The cache, by [`PREVIEW_TTL`] and [`CACHE_MAX_BYTES`], swept at startup
//!   only.
//!
//! Every failure is silent (AC-6). A link that cannot be previewed renders as
//! the plain link it already was.

pub mod cache;
pub mod commands;
pub mod extract;
pub mod net;

use std::path::Path;
use std::time::Duration;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The directory inside `app_config_dir()` that holds the cache.
///
/// The config directory rather than a space's `.copper.assets` sidecar, for
/// three reasons that all point the same way: a preview is derived from a
/// third-party page rather than from user content, so writing one into a
/// git-tracked sidecar commits remote bytes into the user's repository; the
/// cache is naturally per-URL and cross-space, and a per-space copy would fetch
/// the same URL once per space; and a preview is re-derivable where an
/// attachment blob is not, so it can simply be deleted and needs none of the
/// sweep-and-quarantine machinery that exists because blobs cannot.
pub const CACHE_DIR: &str = "previews";

/// How long a cached preview is used before it is fetched again.
///
/// Open Graph data goes stale — a title is edited, an article is retitled — but
/// not quickly, and every re-fetch is another disclosure. A week is long enough
/// that re-reading the same note costs no traffic at all and short enough that a
/// preview does not outlive the page it describes by months.
pub const PREVIEW_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The whole cache's ceiling, enforced oldest-first at startup.
///
/// A preview is a few hundred bytes of JSON plus a thumbnail bounded by
/// `THUMB_MAX_EDGE`, so this is thousands of entries — far more than a person
/// accumulates — and it exists to bound the pathological case rather than the
/// ordinary one.
pub const CACHE_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// What one link's card shows.
///
/// `image` is a **cache filename** for [`commands::preview_image`], never a
/// remote URL. That is the whole difference between this design and one that
/// hotlinks: the WebView is never told where the picture came from and never
/// asks the third party for it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinkPreview {
	/// The href exactly as markdown-it emitted it, which is the key the frontend
	/// asked with and the string it matches the answer back to.
	pub url: String,
	pub site_name: Option<String>,
	pub title: Option<String>,
	pub description: Option<String>,
	pub image: Option<String>,
}

impl LinkPreview {
	/// Whether there is enough here to be worth a card.
	///
	/// A hostname on its own is not: the link is already visible directly above
	/// the card, so a box repeating its domain adds a row of furniture and no
	/// information. A preview that fails this is still **written to the cache** —
	/// "this page carries no metadata" is a stable fact about the page, and
	/// re-fetching to rediscover it every time the note is read would be the one
	/// avoidable disclosure in the whole design.
	pub fn worth_showing(&self) -> bool {
		self.title.is_some() || self.description.is_some() || self.image.is_some()
	}
}

/// Where a page's bytes come from.
///
/// A trait rather than a `reqwest` call inline, and it is the seam the tests in
/// this module stand on: the consent gate, the cache and the extraction rules
/// are all reached through [`preview`], and without this every one of those
/// tests would need a live host to answer. [`net::Web`] is the only production
/// implementation.
pub trait Pages: Send + Sync {
	fn page<'a>(&'a self, url: &'a Url) -> Pending<'a, Option<Page>>;
	fn image<'a>(&'a self, url: &'a Url) -> Pending<'a, Option<Vec<u8>>>;
}

/// A boxed future, spelled out rather than pulled from `futures`: the trait
/// above needs exactly one and the crate is not otherwise a dependency.
pub type Pending<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// A fetched page, decoded.
pub struct Page {
	pub html: String,
	/// Where the response actually came from, after redirects — which is what a
	/// relative `og:image` has to be resolved against, not the URL we asked for.
	pub url: Url,
}

/// The preview for `raw`, from the cache in `dir` when there is one and from the
/// network when there is not.
///
/// `consented` is **asked again at every step that would disclose or record
/// something**, not read once at the top. A page fetch takes seconds against a
/// slow host, and the switch is one click away in a panel the user is looking at
/// while it runs — so a single read at entry would let a withdrawal be followed
/// by a second request to a second host (the `og:image`) and by two cache entries
/// recording that the page was read. Each re-read is a store-side `bool`, taken
/// and released without a lock crossing an `await`.
///
/// The first call is still the only early return that happens before anything is
/// looked at, which is what makes AC-7 structural: with the toggle off this
/// touches neither the network nor the cache, whatever the caller asks for.
///
/// Withdrawal mid-flight returns `None` silently, like every other failure here
/// (AC-6) — and deliberately caches nothing, so switching back on re-asks rather
/// than serving a page from an entry written after consent was taken away.
///
/// **Every step that is not the request itself runs in `spawn_blocking`.** An
/// html5ever parse and an image decode are CPU-bound, and the async runtime's
/// workers are shared with every capture in the app — the rule each attachment
/// command already applies, for the same reason it applies there.
pub async fn preview(
	dir: &Path,
	consented: &(dyn Fn() -> bool + Sync),
	raw: &str,
	pages: &dyn Pages,
) -> Option<LinkPreview> {
	if !consented() {
		return None;
	}

	let url = vet(raw)?;
	let dir = dir.to_path_buf();
	let key = cache_key(&url);

	let hit = blocking({
		let (dir, key) = (dir.clone(), key.clone());
		move || cache::read(&dir, &key)
	})
	.await?;
	if let Some(hit) = hit {
		return hit.worth_showing().then_some(hit);
	}

	// A fetch that failed is deliberately **not** cached below. The stable facts
	// about a page are worth remembering; "the network was down at 9am" is not,
	// and writing it would make one read in flight mode cost the user previews for
	// a week.
	let page = pages.page(&url).await?;
	let meta = blocking(move || extract::extract(&page.html, &page.url)).await?;

	// The page leg is over; the image leg is a request to a *second* host, often a
	// CDN the note never named. Whatever the user did with the toggle while the
	// first request was outstanding decides whether it happens.
	if !consented() {
		return None;
	}

	let downloaded = match meta.image.as_deref().and_then(vet) {
		Some(source) => pages.image(&source).await,
		None => None,
	};

	// Asked once more before anything is written down. A cache entry is a record
	// that this URL was fetched and it is what makes the card come back instantly
	// when previews are switched on again — neither belongs to a read the user has
	// since withdrawn consent for.
	if !consented() {
		return None;
	}

	let built = blocking({
		let host = url.host_str().map(str::to_string);
		let url = raw.trim().to_string();
		move || {
			let image = downloaded.and_then(|bytes| cache::write_image(&dir, &key, &bytes));
			let built = LinkPreview {
				url,
				// The host is a fallback and not a reason to show a card on its own —
				// see `worth_showing`, which is applied to the result below.
				site_name: meta.site_name.or(host),
				title: meta.title,
				description: meta.description,
				image,
			};
			cache::write(&dir, &key, &built);
			built
		}
	})
	.await?;

	built.worth_showing().then_some(built)
}

/// CPU-bound or disk-bound work, off the async runtime's workers.
///
/// A join failure means the closure panicked, and the honest response to that is
/// this feature's response to everything else: no preview, silently.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
	tauri::async_runtime::spawn_blocking(work).await.ok()
}

/// The URL a preview may be fetched for, or nothing.
///
/// The frontend only ever asks about hrefs that already passed `isSafeHref` at
/// render time, so this is the second gate rather than the first — but it is the
/// one that runs in the process doing the fetching, and it refuses three things
/// that allowlist deliberately permits:
///
/// - **`mailto:`**, which is a safe thing to *click* and not a thing to fetch.
/// - **Credentials in the URL.** `https://user:token@host/` would send the
///   secret a note happens to carry to the host, unasked, on render.
/// - **Loopback, private, link-local and unspecified addresses.** A `.copper`
///   can arrive from a git remote, so a note naming `http://192.168.1.1/reboot`
///   or `http://169.254.169.254/…` is a request someone else wrote and this
///   process would make from inside the user's network. Refusing costs intranet
///   links their preview, which is the safe direction to fail in.
///
/// **What this cannot see is DNS.** A public name that resolves to a private
/// address passes here; the connection is not inspected. [`net`] re-applies this
/// to every redirect hop, which closes the shortener-to-loopback route, but a
/// hostile authoritative nameserver is out of reach of a URL check and is
/// accepted as a limitation.
pub fn vet(raw: &str) -> Option<Url> {
	let url = Url::parse(raw.trim()).ok()?;
	if !matches!(url.scheme(), "http" | "https") {
		return None;
	}
	if !url.username().is_empty() || url.password().is_some() {
		return None;
	}
	if !is_public_host(&url) {
		return None;
	}
	Some(url)
}

/// Whether the host is one this process is willing to make a request to.
///
/// Read from `host_str` and classified here rather than through the `url`
/// crate's own `Host` enum, which reqwest does not re-export — so reaching it
/// would mean declaring `url` as a second direct dependency for one match arm.
///
/// The name check is not redundant beside the address check: `localhost` and
/// anything under `.localhost` are *required* to resolve to loopback, and both
/// arrive as names rather than as literals.
pub fn is_public_host(url: &Url) -> bool {
	use std::net::IpAddr;

	let Some(host) = url.host_str() else {
		return false;
	};
	// `host_str` brackets an IPv6 literal, as the URL syntax does.
	let bracketed = host.strip_prefix('[').and_then(|rest| rest.strip_suffix(']'));
	let text = bracketed.unwrap_or(host);

	match text.parse::<IpAddr>() {
		Ok(IpAddr::V4(address)) => is_public_v4(address),
		// `::ffff:127.0.0.1` is loopback wearing an IPv6 spelling, and none of the
		// v6 predicates below say so — so a mapped address is judged as the v4 one
		// it actually is.
		Ok(IpAddr::V6(address)) => match address.to_ipv4_mapped() {
			Some(mapped) => is_public_v4(mapped),
			None => is_public_v6(address),
		},
		// A bracketed host that is not an address is not a host at all.
		Err(_) if bracketed.is_some() => false,
		Err(_) => {
			let name = text.trim_end_matches('.').to_ascii_lowercase();
			!name.is_empty() && name != "localhost" && !name.ends_with(".localhost")
		}
	}
}

fn is_public_v4(address: std::net::Ipv4Addr) -> bool {
	!address.is_loopback()
		&& !address.is_private()
		&& !address.is_link_local()
		&& !address.is_unspecified()
		&& !address.is_broadcast()
		&& !address.is_multicast()
		// `100.64.0.0/10`, carrier-grade NAT: not private by the standard predicate
		// and not somewhere a note should be able to send this process either.
		&& !(address.octets()[0] == 100 && (64..128).contains(&address.octets()[1]))
}

fn is_public_v6(address: std::net::Ipv6Addr) -> bool {
	!address.is_loopback()
		&& !address.is_unspecified()
		&& !address.is_multicast()
		// `fc00::/7`, unique local, and `fe80::/10`, link local. Neither has a
		// predicate on stable Rust, so the prefixes are read directly.
		&& (address.segments()[0] & 0xfe00) != 0xfc00
		&& (address.segments()[0] & 0xffc0) != 0xfe80
}

/// The cache key: the first 16 hex characters of the SHA-256 of the normalised
/// URL.
///
/// **The fragment is dropped and the query is kept.** A `#anchor` cannot change
/// what the page's `<head>` says, so two links differing only there are one
/// preview; a query string routinely *is* the address — `?v=`, `?id=`, `?p=` —
/// so dropping it would serve one page's preview for a hundred different pages.
/// The host arrives lowercased by `Url::parse` already.
/// Named through `attachments::hex16` rather than by a second `format!` here, so
/// the two directories Copper owns stay recognisably the same shape.
pub fn cache_key(url: &Url) -> String {
	let mut normalised = url.clone();
	normalised.set_fragment(None);
	copper_core::attachments::hex16(&Sha256::digest(normalised.as_str().as_bytes()))
}

#[cfg(test)]
mod tests {
	use std::path::PathBuf;

	use super::*;

	/// A source that fails the test if it is ever reached. Every assertion about
	/// the consent gate is really an assertion that this was not called.
	struct NeverAsked;

	impl Pages for NeverAsked {
		fn page<'a>(&'a self, _url: &'a Url) -> Pending<'a, Option<Page>> {
			panic!("a page was fetched when nothing should have been");
		}

		fn image<'a>(&'a self, _url: &'a Url) -> Pending<'a, Option<Vec<u8>>> {
			panic!("an image was fetched when nothing should have been");
		}
	}

	/// One canned page, and a count of how many times each leg was asked for.
	struct Canned {
		html: String,
		image: Option<Vec<u8>>,
		pages: std::sync::atomic::AtomicUsize,
		images: std::sync::atomic::AtomicUsize,
	}

	impl Canned {
		fn new(html: &str) -> Self {
			Self {
				html: html.to_string(),
				image: None,
				pages: std::sync::atomic::AtomicUsize::new(0),
				images: std::sync::atomic::AtomicUsize::new(0),
			}
		}

		fn with_image(html: &str, image: Vec<u8>) -> Self {
			Self {
				image: Some(image),
				..Self::new(html)
			}
		}

		fn fetches(&self) -> usize {
			self.pages.load(std::sync::atomic::Ordering::Relaxed)
		}

		fn image_fetches(&self) -> usize {
			self.images.load(std::sync::atomic::Ordering::Relaxed)
		}
	}

	impl Pages for Canned {
		fn page<'a>(&'a self, url: &'a Url) -> Pending<'a, Option<Page>> {
			self.pages.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
			let page = Page {
				html: self.html.clone(),
				url: url.clone(),
			};
			Box::pin(async move { Some(page) })
		}

		fn image<'a>(&'a self, _url: &'a Url) -> Pending<'a, Option<Vec<u8>>> {
			self.images.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
			let bytes = self.image.clone();
			Box::pin(async move { bytes })
		}
	}

	/// Every failure this module has is silent, so a source that answers nothing
	/// is the shape of an unreachable host, a timeout and a refused
	/// `Content-Type` alike.
	struct Silent;

	impl Pages for Silent {
		fn page<'a>(&'a self, _url: &'a Url) -> Pending<'a, Option<Page>> {
			Box::pin(async { None })
		}

		fn image<'a>(&'a self, _url: &'a Url) -> Pending<'a, Option<Vec<u8>>> {
			Box::pin(async { None })
		}
	}

	fn block<F: std::future::Future>(task: F) -> F::Output {
		tauri::async_runtime::block_on(task)
	}

	/// The cache directory a test runs against — inside a `tempfile::tempdir()`,
	/// never the real one under `app_config_dir()`.
	fn cache_dir(dir: &tempfile::TempDir) -> PathBuf {
		dir.path().join(CACHE_DIR)
	}

	const PAGE: &str = r#"<html><head>
		<meta property="og:title" content="A title">
		<meta property="og:description" content="A description">
		<meta property="og:site_name" content="Example">
	</head></html>"#;

	/// AC-7, and the reason the flag is a parameter rather than something the
	/// frontend decides: with it false there is no path through this function
	/// that reaches the network *or* the disk.
	#[test]
	fn nothing_is_fetched_or_read_when_the_toggle_is_off() {
		let dir = tempfile::tempdir().unwrap();
		let cache = cache_dir(&dir);

		let answer = block(preview(&cache, &|| false, "https://example.com/", &NeverAsked));

		assert_eq!(answer, None);
		assert!(!cache.exists(), "the disabled path touched the cache directory");
	}

	/// Consent withdrawn while the page request was outstanding stops everything
	/// that had not happened yet: the `og:image` request to a second host, and both
	/// cache entries. The page fetch itself is already gone and cannot be recalled,
	/// which is exactly why the later steps have to be asked about separately.
	#[test]
	fn withdrawing_consent_mid_fetch_stops_the_image_leg_and_the_cache_write() {
		let dir = tempfile::tempdir().unwrap();
		let cache = cache_dir(&dir);
		let html = r#"<meta property="og:title" content="A title">
			<meta property="og:image" content="https://cdn.example.com/hero.png">"#;
		let source = Canned::with_image(html, wide_png());

		// True for the entry check, false from the moment the page leg is over —
		// the toggle being switched off while a slow host was still answering.
		let asked = std::sync::atomic::AtomicUsize::new(0);
		let consented = || asked.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0;

		assert_eq!(block(preview(&cache, &consented, "https://example.com/", &source)), None);

		assert_eq!(source.fetches(), 1, "the page leg should have run before the withdrawal");
		assert_eq!(source.image_fetches(), 0, "the image was fetched after consent was withdrawn");
		assert!(!cache.exists(), "the withdrawn fetch was written to the cache");
	}

	/// AC-3 and AC-5 together: the first read fetches, and every read after it is
	/// served from disk. The count is the assertion — a cache that is written but
	/// never consulted looks identical from the outside.
	#[test]
	fn a_url_is_fetched_once_and_then_served_from_the_cache() {
		let dir = tempfile::tempdir().unwrap();
		let cache = cache_dir(&dir);
		let source = Canned::new(PAGE);

		let first = block(preview(&cache, &|| true, "https://example.com/a", &source)).unwrap();
		let second = block(preview(&cache, &|| true, "https://example.com/a", &source)).unwrap();

		assert_eq!(first, second);
		assert_eq!(first.title.as_deref(), Some("A title"));
		assert_eq!(first.description.as_deref(), Some("A description"));
		assert_eq!(first.site_name.as_deref(), Some("Example"));
		assert_eq!(source.fetches(), 1, "the second read went back to the network");
	}

	/// Two links differing only by their fragment are one page and must be one
	/// fetch; two differing by their query are two pages and must be two.
	#[test]
	fn the_key_ignores_a_fragment_and_respects_a_query() {
		let dir = tempfile::tempdir().unwrap();
		let cache = cache_dir(&dir);
		let source = Canned::new(PAGE);

		block(preview(&cache, &|| true, "https://example.com/a?v=1", &source));
		block(preview(&cache, &|| true, "https://example.com/a?v=1#top", &source));
		assert_eq!(source.fetches(), 1, "a fragment was treated as a different page");

		block(preview(&cache, &|| true, "https://example.com/a?v=2", &source));
		assert_eq!(source.fetches(), 2, "a query was treated as the same page");
	}

	/// A page with nothing to say is remembered as such. The card is not shown,
	/// and — the half that matters — the page is not asked again.
	#[test]
	fn a_page_with_no_metadata_is_cached_as_nothing_rather_than_refetched() {
		let dir = tempfile::tempdir().unwrap();
		let cache = cache_dir(&dir);
		let source = Canned::new("<html><head></head><body>hello</body></html>");

		assert_eq!(block(preview(&cache, &|| true, "https://example.com/", &source)), None);
		assert_eq!(block(preview(&cache, &|| true, "https://example.com/", &source)), None);
		assert_eq!(source.fetches(), 1, "an empty page was fetched twice");
	}

	/// The opposite rule, and the reason the two cases are not one: a fetch that
	/// failed says nothing about the page, so caching it would make a single
	/// offline read cost the user previews until the entry expired.
	#[test]
	fn a_failed_fetch_is_silent_and_leaves_nothing_behind_to_retry_against() {
		let dir = tempfile::tempdir().unwrap();
		let cache = cache_dir(&dir);

		assert_eq!(block(preview(&cache, &|| true, "https://example.com/", &Silent)), None);

		let source = Canned::new(PAGE);
		let recovered = block(preview(&cache, &|| true, "https://example.com/", &source));
		assert!(recovered.is_some(), "the failure was cached and blocked a later success");
	}

	#[test]
	fn an_image_is_downscaled_into_the_cache_and_named_by_key() {
		let dir = tempfile::tempdir().unwrap();
		let cache = cache_dir(&dir);
		let html = r#"<meta property="og:title" content="A title">
			<meta property="og:image" content="https://cdn.example.com/hero.png">"#;
		let source = Canned::with_image(html, wide_png());

		let built = block(preview(&cache, &|| true, "https://example.com/", &source)).unwrap();

		let file = built.image.expect("the image should have been stored");
		let stored = cache.join(&file);
		assert!(stored.is_file(), "{file} was not written");
		// Downscaled rather than stored as it arrived: the ceiling is
		// `THUMB_MAX_EDGE`, inherited from the attachment thumbnail path.
		let (width, _) = crate::attachments::thumb::dimensions(
			&std::fs::read(&stored).unwrap(),
			"image/png",
		);
		assert_eq!(width, Some(crate::attachments::thumb::THUMB_MAX_EDGE));
	}

	// --- vet ---

	#[test]
	fn only_fetchable_public_http_urls_are_accepted() {
		for raw in [
			"https://example.com/a",
			"http://example.com/",
			"https://example.com:8443/x?y=1#z",
			"https://8.8.8.8/",
		] {
			assert!(vet(raw).is_some(), "{raw} should be fetchable");
		}
	}

	#[test]
	fn schemes_credentials_and_private_hosts_are_refused() {
		for raw in [
			// Passes `isSafeHref` on the render side and is deliberately not
			// fetchable: clicking a mail address is not asking to load a page.
			"mailto:someone@example.com",
			"ftp://example.com/",
			"file:///C:/Windows/win.ini",
			"not a url at all",
			"https://",
			// The secret a note happens to carry must not be posted to the host.
			"https://user:token@example.com/",
			"https://user@example.com/",
			// The router, the metadata service, and the machine this is running on.
			"http://127.0.0.1:1420/",
			"http://localhost/",
			"http://api.LOCALHOST/",
			"http://192.168.1.1/",
			"http://10.0.0.5/",
			"http://172.16.4.4/",
			"http://169.254.169.254/latest/meta-data/",
			"http://100.100.0.1/",
			"http://0.0.0.0/",
			"http://[::1]/",
			"http://[fe80::1]/",
			"http://[fd00::1]/",
			"http://[::ffff:127.0.0.1]/",
		] {
			assert!(vet(raw).is_none(), "{raw} should not be fetchable");
		}
	}

	/// A 640×320 PNG, wider than the thumbnail box on purpose so the downscale is
	/// observable.
	fn wide_png() -> Vec<u8> {
		let mut buffer = std::io::Cursor::new(Vec::new());
		image::DynamicImage::new_rgb8(640, 320)
			.write_to(&mut buffer, image::ImageFormat::Png)
			.unwrap();
		buffer.into_inner()
	}
}
