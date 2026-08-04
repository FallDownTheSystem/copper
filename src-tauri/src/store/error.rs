//! The one error type the store returns, and the shape it takes over IPC.
//!
//! Every variant carries a message meant to be read by a person; the variant
//! itself is what the frontend branches on. Serialising as `{ kind, message }`
//! (spec 8.6) exists so Phase 3 never has to string-match an error to tell a
//! missing file from a write conflict.

use serde::ser::{Serialize, SerializeStruct, Serializer};

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum StoreError {
	/// An id, path or section that the caller named does not exist.
	#[error("{0}")]
	NotFound(String),
	/// The filesystem refused, or the bytes could not be read or written.
	#[error("{0}")]
	Io(String),
	/// The bytes were read but are not a document this store can understand.
	#[error("{0}")]
	Parse(String),
	/// The file moved under us more times than the write path will retry.
	#[error("{0}")]
	Conflict(String),
	/// The arguments are wrong: empty body, empty name, empty id list.
	#[error("{0}")]
	Invalid(String),
	/// The store is in a state that cannot serve this call — no space open, or
	/// the open space's file is currently unreadable.
	#[error("{0}")]
	Unavailable(String),
}

impl StoreError {
	/// The stable, lowercase-kebab discriminant the frontend branches on.
	pub fn kind(&self) -> &'static str {
		match self {
			Self::NotFound(_) => "not-found",
			Self::Io(_) => "io",
			Self::Parse(_) => "parse",
			Self::Conflict(_) => "conflict",
			Self::Invalid(_) => "invalid",
			Self::Unavailable(_) => "unavailable",
		}
	}

	/// The human-readable half.
	pub fn message(&self) -> String {
		self.to_string()
	}
}

/// Hand-written rather than derived: the derive would emit an externally tagged
/// enum (`{"NotFound": "..."}`), and the contract in spec 8.6 is a flat
/// `{ kind, message }` object that is the same shape for every variant.
impl Serialize for StoreError {
	fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
		let mut state = serializer.serialize_struct("StoreError", 2)?;
		state.serialize_field("kind", self.kind())?;
		state.serialize_field("message", &self.message())?;
		state.end()
	}
}

/// Attaches the path to an `io::Error`, because "Access is denied. (os error 5)"
/// on its own is not actionable.
pub fn io_err(path: &std::path::Path, action: &str, err: &std::io::Error) -> StoreError {
	let message = format!("could not {action} {}: {err}", path.display());
	// A missing file is a distinct case for callers — startup's recents fallback
	// (spec 7.3) has to tell "not checked out right now" from "permission denied".
	if err.kind() == std::io::ErrorKind::NotFound {
		StoreError::NotFound(message)
	} else {
		StoreError::Io(message)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn serialises_as_kind_and_message() {
		let json = serde_json::to_string(&StoreError::Conflict("busy".into())).unwrap();
		assert_eq!(json, r#"{"kind":"conflict","message":"busy"}"#);
	}

	#[test]
	fn every_kind_is_lowercase_kebab() {
		let all = [
			StoreError::NotFound(String::new()),
			StoreError::Io(String::new()),
			StoreError::Parse(String::new()),
			StoreError::Conflict(String::new()),
			StoreError::Invalid(String::new()),
			StoreError::Unavailable(String::new()),
		];
		for err in all {
			let kind = err.kind();
			assert!(
				kind.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
				"{kind} is not lowercase kebab"
			);
		}
	}

	#[test]
	fn missing_file_maps_to_not_found() {
		let err = std::io::Error::new(std::io::ErrorKind::NotFound, "nope");
		let mapped = io_err(std::path::Path::new("C:\\x.copper"), "read", &err);
		assert_eq!(mapped.kind(), "not-found");
		assert!(mapped.message().contains("x.copper"));
	}
}
