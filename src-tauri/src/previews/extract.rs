//! Open Graph extraction: HTML in, four optional strings out.
//!
//! **A real parser, not a regex over `<meta>`.** `dom_query` is html5ever
//! underneath, which decodes character references while parsing — so a title of
//! `Ben &amp; Jerry&#39;s` arrives as text rather than as markup — and it is
//! immune to the four things a pattern over `<meta …>` gets wrong in the wild:
//! attribute order, `name=` where the specification says `property=`, unquoted
//! values, and a `<meta>` that is inside a comment or a `<script>` body and is
//! therefore not a tag at all.
//!
//! Every tag is read **once, into a map**, rather than through a selector per
//! field. Ten selector parses over one document is the smaller reason; the
//! larger one is that CSS attribute matching is case-sensitive for an unknown
//! attribute, so `<meta property="OG:TITLE">` would silently miss, and a map
//! keyed on the lowercased name has no such edge.

use std::collections::HashMap;

use dom_query::Document;
use reqwest::Url;

/// What a page said about itself.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Meta {
	pub title: Option<String>,
	pub description: Option<String>,
	pub site_name: Option<String>,
	/// Absolute, resolved against the page's own URL.
	pub image: Option<String>,
}

/// The ceiling on any one field.
///
/// A `<meta content="…">` can hold the whole response body, and whatever is in
/// it would otherwise be written to the cache and sent over IPC. Three hundred
/// characters is more than a card can show and far less than a page can carry.
const TEXT_MAX_CHARS: usize = 300;

/// Reads the four fields, each through its own fallback chain.
///
/// The chains are what make this work on the real web rather than on the
/// specification: Twitter's card tags are as common as Open Graph's, and a page
/// with neither still has a `<title>` and usually an icon.
///
/// | Field | Order |
/// | --- | --- |
/// | title | `og:title` → `twitter:title` → `<title>` |
/// | description | `og:description` → `twitter:description` → `description` |
/// | image | `og:image` → `twitter:image` → `apple-touch-icon` → `icon` |
/// | site name | `og:site_name` → the caller's fallback to the host |
pub fn extract(html: &str, url: &Url) -> Meta {
	let document = Document::from(html);
	let meta = meta_tags(&document);
	let pick = |keys: &[&str]| keys.iter().find_map(|key| meta.get(*key).cloned());

	Meta {
		title: pick(&["og:title", "twitter:title"]).or_else(|| {
			// `select_single` rather than `select`: a page can carry a `<title>` inside
			// an inline SVG, and the document's own is the first one.
			tidy(&document.select_single("title").text())
		}),
		description: pick(&["og:description", "twitter:description", "description"]),
		site_name: pick(&["og:site_name", "application-name"]),
		image: pick(&["og:image", "og:image:url", "twitter:image", "twitter:image:src"])
			.or_else(|| icon(&document))
			// Resolved against the page rather than sent on as written: `og:image` is
			// routinely a root-relative path, and the *final* URL after redirects is
			// what it is relative to.
			.and_then(|raw| url.join(&raw).ok())
			.map(|resolved| resolved.to_string()),
	}
}

/// Every `<meta>` with a name and content, keyed on the lowercased name.
///
/// **Both `property=` and `name=` are read.** The Open Graph specification says
/// `property`, HTML's own validator prefers `name`, and enough real sites emit
/// `<meta name="og:title">` that reading only one of them loses previews for no
/// reason. The first tag wins, matching how a browser resolves a duplicate.
fn meta_tags(document: &Document) -> HashMap<String, String> {
	let mut found: HashMap<String, String> = HashMap::new();
	for node in document.select("meta").nodes() {
		let Some(key) = node.attr("property").or_else(|| node.attr("name")) else {
			continue;
		};
		let Some(content) = node.attr("content").as_deref().and_then(tidy) else {
			continue;
		};
		found
			.entry(key.trim().to_ascii_lowercase())
			.or_insert(content);
	}
	found
}

/// The page's icon, as the last resort before there is no picture at all.
///
/// `apple-touch-icon` first because it is the one icon that is specified to be
/// large — 180 square is the usual — where `icon` is often a 16-pixel favicon
/// that says nothing at card size. A `rel` may list several tokens, so it is
/// split rather than compared whole.
fn icon(document: &Document) -> Option<String> {
	for wanted in ["apple-touch-icon", "apple-touch-icon-precomposed", "icon"] {
		for node in document.select("link").nodes() {
			let Some(rel) = node.attr("rel") else { continue };
			if !rel
				.split_ascii_whitespace()
				.any(|token| token.eq_ignore_ascii_case(wanted))
			{
				continue;
			}
			if let Some(href) = node.attr("href").as_deref().and_then(tidy) {
				return Some(href);
			}
		}
	}
	None
}

/// Collapses whitespace and bounds the length, or `None` for a field that turns
/// out to be empty.
///
/// The collapse is not cosmetic: an `og:description` is regularly a pretty-
/// printed paragraph with newlines and tab indentation in it, and a card is one
/// line.
fn tidy(raw: &str) -> Option<String> {
	let mut collapsed = String::new();
	for word in raw.split_whitespace() {
		if !collapsed.is_empty() {
			collapsed.push(' ');
		}
		collapsed.push_str(word);
	}
	if collapsed.is_empty() {
		return None;
	}
	// By characters rather than bytes: truncating a UTF-8 string on a byte index
	// panics mid-codepoint, and a page's title is exactly where a multi-byte
	// character turns up.
	Some(collapsed.chars().take(TEXT_MAX_CHARS).collect())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn page() -> Url {
		Url::parse("https://example.com/articles/one").unwrap()
	}

	fn extract_from(html: &str) -> Meta {
		extract(html, &page())
	}

	#[test]
	fn open_graph_tags_are_read_in_full() {
		let meta = extract_from(
			r#"<html><head>
				<meta property="og:title" content="The title">
				<meta property="og:description" content="The description">
				<meta property="og:site_name" content="Example">
				<meta property="og:image" content="https://cdn.example.com/hero.png">
			</head></html>"#,
		);

		assert_eq!(meta.title.as_deref(), Some("The title"));
		assert_eq!(meta.description.as_deref(), Some("The description"));
		assert_eq!(meta.site_name.as_deref(), Some("Example"));
		assert_eq!(meta.image.as_deref(), Some("https://cdn.example.com/hero.png"));
	}

	/// Plenty of real sites emit `name=` where the specification says
	/// `property=`, and reading only one of the two loses those previews for a
	/// reason nobody could see from the outside.
	#[test]
	fn a_name_attribute_is_read_like_a_property_one() {
		let meta = extract_from(r#"<meta name="og:title" content="Named not propertied">"#);
		assert_eq!(meta.title.as_deref(), Some("Named not propertied"));
	}

	#[test]
	fn the_key_is_matched_whatever_case_it_is_written_in() {
		let meta = extract_from(r#"<meta property="OG:Title" content="Shouted">"#);
		assert_eq!(meta.title.as_deref(), Some("Shouted"));
	}

	#[test]
	fn each_field_falls_back_through_its_chain() {
		let twitter = extract_from(
			r#"<title>The tab</title>
			   <meta name="twitter:title" content="The card">
			   <meta name="twitter:description" content="Carded">
			   <meta name="twitter:image" content="https://cdn.example.com/card.png">"#,
		);
		assert_eq!(twitter.title.as_deref(), Some("The card"));
		assert_eq!(twitter.description.as_deref(), Some("Carded"));
		assert_eq!(twitter.image.as_deref(), Some("https://cdn.example.com/card.png"));

		let plain = extract_from(
			r#"<html><head><title>The tab</title>
			   <meta name="description" content="Plainly described">
			   <link rel="apple-touch-icon" href="/touch.png"></head></html>"#,
		);
		assert_eq!(plain.title.as_deref(), Some("The tab"));
		assert_eq!(plain.description.as_deref(), Some("Plainly described"));
		assert_eq!(plain.image.as_deref(), Some("https://example.com/touch.png"));
	}

	/// A root-relative `og:image` is the ordinary case, and the URL it is
	/// relative to is the page's own.
	#[test]
	fn a_relative_image_is_resolved_against_the_page() {
		for (written, expected) in [
			("/hero.png", "https://example.com/hero.png"),
			("hero.png", "https://example.com/articles/hero.png"),
			("//cdn.example.com/hero.png", "https://cdn.example.com/hero.png"),
		] {
			let meta = extract_from(&format!(r#"<meta property="og:image" content="{written}">"#));
			assert_eq!(meta.image.as_deref(), Some(expected), "{written}");
		}
	}

	/// The half a regex cannot do. html5ever resolves the entity while parsing;
	/// a pattern match hands `&amp;` straight through to the card.
	#[test]
	fn character_references_are_decoded_rather_than_shown() {
		let meta = extract_from(r#"<meta property="og:title" content="Ben &amp; Jerry&#39;s">"#);
		assert_eq!(meta.title.as_deref(), Some("Ben & Jerry's"));
	}

	/// The other half. A `<meta>` inside a comment or a script body is text, not
	/// a tag, and a pattern over the source cannot tell the difference.
	#[test]
	fn a_meta_tag_that_is_not_a_tag_is_not_read() {
		let commented = extract_from(
			r#"<!-- <meta property="og:title" content="Commented out"> -->
			   <meta property="og:title" content="The real one">"#,
		);
		assert_eq!(commented.title.as_deref(), Some("The real one"));

		let scripted =
			extract_from(r#"<script>var s = '<meta property="og:title" content="Injected">';</script>"#);
		assert_eq!(scripted.title, None);
	}

	#[test]
	fn whitespace_is_collapsed_and_a_long_field_is_bounded() {
		let meta = extract_from(
			"<meta property=\"og:description\" content=\"  wrapped\n\t\tover   lines  \">",
		);
		assert_eq!(meta.description.as_deref(), Some("wrapped over lines"));

		let long = "é".repeat(TEXT_MAX_CHARS * 2);
		let bounded = extract_from(&format!(r#"<meta property="og:title" content="{long}">"#));
		// By characters, not bytes: a byte-index truncation panics inside a
		// multi-byte character, which is exactly what a page title contains.
		assert_eq!(bounded.title.unwrap().chars().count(), TEXT_MAX_CHARS);
	}

	#[test]
	fn an_empty_or_whitespace_only_field_is_absent_rather_than_blank() {
		let meta = extract_from(
			r#"<meta property="og:title" content="   ">
			   <meta property="og:description" content="">"#,
		);
		assert_eq!(meta.title, None);
		assert_eq!(meta.description, None);
	}

	#[test]
	fn a_page_with_nothing_to_say_yields_nothing() {
		assert_eq!(extract_from("<html><body>hello</body></html>"), Meta::default());
		assert_eq!(extract_from(""), Meta::default());
		// Not HTML at all, which is what a mislabelled response body looks like.
		assert_eq!(extract_from("\u{0}\u{1}\u{2}not markup"), Meta::default());
	}

	/// The first tag wins, the way a browser resolves a duplicate — and the way
	/// that matters, because a page that sets `og:title` twice is not asking for
	/// the last one.
	#[test]
	fn the_first_of_a_duplicated_tag_is_the_one_used() {
		let meta = extract_from(
			r#"<meta property="og:title" content="First">
			   <meta property="og:title" content="Second">"#,
		);
		assert_eq!(meta.title.as_deref(), Some("First"));
	}
}
