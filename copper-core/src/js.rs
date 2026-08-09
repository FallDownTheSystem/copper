//! JavaScript's character classes, for the two modules that are ports of it.
//!
//! `markdown` and `search` both claim to produce what the TypeScript they were
//! ported from produces, and both decide things by asking "is this whitespace".
//! The two languages disagree about that, so the answer is written out once here
//! rather than approximated twice.

/// JavaScript's `\s`, which is **not** `char::is_whitespace()`.
///
/// Two differences, and each would be silent:
///
/// - `\s` includes U+FEFF, the byte-order mark. Rust's `White_Space` property
///   does not, so a body beginning with a BOM would split differently.
/// - Rust counts U+0085 (NEL) as whitespace. `\s` does not.
///
/// Everything else lines up: `\s` is `White_Space` minus NEL, plus the two line
/// terminators U+2028/U+2029 (already in `White_Space`) and the BOM.
pub(crate) fn is_whitespace(ch: char) -> bool {
	matches!(
		ch,
		'\u{9}'..='\u{d}'
			| '\u{20}'
			| '\u{a0}'
			| '\u{1680}'
			| '\u{2000}'..='\u{200a}'
			| '\u{2028}'
			| '\u{2029}'
			| '\u{202f}'
			| '\u{205f}'
			| '\u{3000}'
			| '\u{feff}'
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_two_characters_rust_and_javascript_disagree_about() {
		assert!(is_whitespace('\u{feff}'), "the BOM is whitespace to a JS regex");
		assert!(!'\u{feff}'.is_whitespace(), "…and is not to Rust, which is the point");

		assert!(!is_whitespace('\u{85}'), "NEL is not whitespace to a JS regex");
		assert!('\u{85}'.is_whitespace(), "…and is to Rust, which is the other point");
	}

	#[test]
	fn the_ordinary_characters_agree() {
		for ch in [' ', '\t', '\n', '\r', '\u{b}', '\u{c}', '\u{a0}', '\u{3000}'] {
			assert!(is_whitespace(ch), "{ch:?}");
		}
		for ch in ['a', '-', '#', '|', '\u{200b}'] {
			assert!(!is_whitespace(ch), "{ch:?}");
		}
	}
}
