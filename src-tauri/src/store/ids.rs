//! Prefixed ids, unique within the document that will hold them.
//!
//! The prefix is not decoration: it makes a hand-edited `.copper` file readable,
//! and it makes a mistyped id in a `section` reference obviously wrong rather
//! than merely dangling. Uniqueness is checked against the target document
//! rather than assumed from the entropy, because a document can also arrive
//! from a merge, a hand edit, or a copy-pasted note.

use uuid::Uuid;

pub const SPACE: &str = "spc";
pub const SECTION: &str = "sec";
pub const NOTE: &str = "nte";

/// `<prefix>_<8 hex chars>` — the shortest form that still satisfies spec 1.3.
pub fn new_id(prefix: &str) -> String {
	let hex = Uuid::new_v4().simple().to_string();
	format!("{prefix}_{}", &hex[..8])
}

/// Regenerates until `exists` says the candidate is free.
///
/// Bounded rather than an open loop: 8 hex characters is 4 billion values, so a
/// collision run this long means `exists` is lying (always true) rather than
/// that we are unlucky, and an unbounded loop would hang the caller instead of
/// producing a usable id. Falling back to the full uuid keeps the id unique
/// where it matters and only costs the short form's readability.
pub fn unique_id(prefix: &str, exists: impl Fn(&str) -> bool) -> String {
	for _ in 0..16 {
		let candidate = new_id(prefix);
		if !exists(&candidate) {
			return candidate;
		}
	}
	format!("{prefix}_{}", Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn has_prefix_and_eight_hex_characters() {
		let id = new_id(NOTE);
		let (prefix, hex) = id.split_once('_').expect("id is prefixed");
		assert_eq!(prefix, "nte");
		assert_eq!(hex.len(), 8);
		assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
	}

	#[test]
	fn retries_past_a_taken_id() {
		let taken = new_id(SECTION);
		let generated = unique_id(SECTION, |candidate| candidate == taken);
		assert_ne!(generated, taken);
	}

	#[test]
	fn falls_back_to_a_long_id_when_everything_collides() {
		let generated = unique_id(SPACE, |_| true);
		assert_eq!(generated.len(), "spc_".len() + 32);
	}

	#[test]
	fn ids_do_not_repeat() {
		let generated: std::collections::HashSet<_> = (0..1000).map(|_| new_id(NOTE)).collect();
		assert_eq!(generated.len(), 1000);
	}
}
