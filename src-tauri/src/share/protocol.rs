//! What a sealed message carries, and the two shapes it takes.
//!
//! Two structs rather than one. [`WirePayload`] is what serde sees — attachment
//! bytes as base64 strings, because the plaintext is JSON. [`Payload`] is what
//! the rest of the feature uses, with the bytes decoded. Keeping them apart puts
//! the base64 at the boundary and nowhere else, so no other file has to remember
//! which form it is holding.
//!
//! ```json
//! {
//!   "v": 1,
//!   "notes": [
//!     { "body": "…", "attachments": [ { "name": "Screenshot.png", "bytes": "<base64>" } ] }
//!   ]
//! }
//! ```
//!
//! Base64 inflates attachment bytes by a third before encryption, so a 20 MiB
//! ciphertext carries roughly 14 MiB of files. That is stated in the settings
//! description and in the over-cap error rather than hidden. What it buys is one
//! code path, a payload a test can read, and no hand-rolled binary framing.
//!
//! **A peer is untrusted input.** Holding the pairing secret proves the message
//! came from the other device; it does not make the bytes well-formed. Every
//! limit the local ingest path enforces is enforced again here, on the way in.

use base64::Engine as _;
use copper_core::attachments::{ATTACHMENT_MAX_BYTES, ATTACHMENT_MAX_PER_NOTE};
use copper_core::store::error::{Result, StoreError};
use serde::{Deserialize, Serialize};

/// The ceiling on a finished ciphertext.
///
/// Below KV's 25 MiB value cap with room for the nonce, the tag and the
/// Worker's own framing. Deliberately **not** [`ATTACHMENT_MAX_BYTES`], which is
/// 32 MiB: the local limit is what this machine will store, and this is what
/// fits through a relay. A note that exceeds it is refused with both numbers
/// named.
pub const SHARE_MAX_PAYLOAD_BYTES: usize = 20 * 1024 * 1024;

/// A 24-byte nonce plus a 16-byte Poly1305 tag. Nothing shorter can be a sealed
/// message, so a shorter one is refused before it reaches the AEAD.
pub const MIN_WIRE_BYTES: usize = 40;

/// The only payload version this build speaks.
const VERSION: u32 = 1;

/// The base64 flavour attachment bytes travel in: standard alphabet, padded.
const BYTES_ENCODING: base64::engine::general_purpose::GeneralPurpose =
	base64::engine::general_purpose::STANDARD;

// --- the wire form -----------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct WirePayload {
	v: u32,
	notes: Vec<WireNote>,
}

#[derive(Serialize, Deserialize)]
struct WireNote {
	body: String,
	attachments: Vec<WireAttachment>,
}

#[derive(Serialize, Deserialize)]
struct WireAttachment {
	name: String,
	bytes: String,
}

// --- the decoded form --------------------------------------------------------

/// One message, as every other file in this module handles it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
	pub notes: Vec<PayloadNote>,
}

/// One note in a message: its body, and its attachments as `(name, bytes)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadNote {
	pub body: String,
	pub attachments: Vec<(String, Vec<u8>)>,
}

/// Serialises a message, base64-encoding attachment bytes on the way out.
pub fn build_payload(notes: &[PayloadNote]) -> Result<Vec<u8>> {
	let wire = WirePayload {
		v: VERSION,
		notes: notes
			.iter()
			.map(|note| WireNote {
				body: note.body.clone(),
				attachments: note
					.attachments
					.iter()
					.map(|(name, bytes)| WireAttachment {
						name: name.clone(),
						bytes: BYTES_ENCODING.encode(bytes),
					})
					.collect(),
			})
			.collect(),
	};

	serde_json::to_vec(&wire)
		.map_err(|err| StoreError::Io(format!("this note could not be prepared for sending: {err}")))
}

/// Parses a decrypted message, refusing anything the local paths would refuse.
///
/// The version is checked first and named in the refusal, because it is the one
/// failure a newer peer can cause that the user can act on — this is the message
/// that has to say "the other machine is running a newer Copper".
pub fn parse_payload(plaintext: &[u8]) -> Result<Payload> {
	let wire: WirePayload = serde_json::from_slice(plaintext)
		.map_err(|err| StoreError::Invalid(format!("this message is not a Copper payload: {err}")))?;

	if wire.v != VERSION {
		return Err(StoreError::Invalid(format!(
			"this message is version {} and this copy of Copper understands version {VERSION}; \
			 update the app on both devices",
			wire.v
		)));
	}
	if wire.notes.is_empty() {
		return Err(StoreError::Invalid("this message carries no notes".into()));
	}

	let mut notes = Vec::with_capacity(wire.notes.len());
	for note in wire.notes {
		// **Refused here rather than left to the store.** `ops::add_note` rejects an
		// empty body unconditionally — even one carrying attachments — and
		// `attachments::ingest` rejects zero bytes. Both are *deterministic*
		// failures, and a delivery failure is not a poison failure: it stops the
		// tick without advancing the counter, so a peer that sent a blank note
		// would block this mailbox for every later sequence, for ever. Catching it
		// at the parse turns "wedged" into "reported and skipped after
		// POISON_LIMIT".
		if note.body.trim().is_empty() {
			return Err(StoreError::Invalid("a note in this message has an empty body".into()));
		}
		if note.attachments.len() > ATTACHMENT_MAX_PER_NOTE {
			return Err(StoreError::Invalid(format!(
				"a note in this message carries {} attachments, and the limit is \
				 {ATTACHMENT_MAX_PER_NOTE}",
				note.attachments.len()
			)));
		}

		let mut attachments = Vec::with_capacity(note.attachments.len());
		for attachment in note.attachments {
			let bytes = BYTES_ENCODING.decode(&attachment.bytes).map_err(|_| {
				StoreError::Invalid(format!(
					"the attachment {} in this message is not readable",
					attachment.name
				))
			})?;
			// Same reasoning as the empty body above: `ingest` refuses zero bytes,
			// deterministically, and a delivery failure does not count as poison.
			if bytes.is_empty() {
				return Err(StoreError::Invalid(format!(
					"the attachment {} in this message is empty",
					attachment.name
				)));
			}
			if bytes.len() as u64 > ATTACHMENT_MAX_BYTES {
				return Err(StoreError::Invalid(format!(
					"the attachment {} in this message is {} bytes, over the {ATTACHMENT_MAX_BYTES} \
					 byte limit",
					attachment.name,
					bytes.len()
				)));
			}
			attachments.push((attachment.name, bytes));
		}

		notes.push(PayloadNote {
			body: note.body,
			attachments,
		});
	}

	Ok(Payload { notes })
}

#[cfg(test)]
mod tests {
	use super::*;

	fn note(body: &str, attachments: Vec<(&str, Vec<u8>)>) -> PayloadNote {
		PayloadNote {
			body: body.into(),
			attachments: attachments
				.into_iter()
				.map(|(name, bytes)| (name.to_string(), bytes))
				.collect(),
		}
	}

	#[test]
	fn a_build_parse_round_trip_preserves_bodies_and_attachment_bytes_exactly() {
		let notes = vec![
			note("first, with *markdown* and a \n newline", Vec::new()),
			note(
				"second",
				vec![("Screenshot.png", vec![0u8, 1, 2, 255, 254]), ("notes.txt", b"hi".to_vec())],
			),
		];

		let built = build_payload(&notes).unwrap();
		let parsed = parse_payload(&built).unwrap();

		assert_eq!(parsed.notes, notes);
	}

	/// The plaintext is inspectable JSON, which is half of why base64 was chosen
	/// over hand-rolled binary framing.
	#[test]
	fn the_plaintext_is_versioned_json() {
		let built = build_payload(&[note("body", Vec::new())]).unwrap();
		let value: serde_json::Value = serde_json::from_slice(&built).unwrap();
		assert_eq!(value["v"], 1);
		assert_eq!(value["notes"][0]["body"], "body");
	}

	#[test]
	fn a_newer_version_is_refused_and_named() {
		let plaintext = br#"{"v":2,"notes":[{"body":"x","attachments":[]}]}"#;
		let err = parse_payload(plaintext).unwrap_err();
		assert_eq!(err.kind(), "invalid");
		assert!(err.message().contains('2'), "{}", err.message());
	}

	#[test]
	fn an_empty_note_list_is_refused() {
		let plaintext = br#"{"v":1,"notes":[]}"#;
		assert_eq!(parse_payload(plaintext).unwrap_err().kind(), "invalid");
	}

	#[test]
	fn malformed_json_is_invalid_rather_than_a_panic() {
		assert!(parse_payload(b"not json").is_err());
		assert!(parse_payload(b"").is_err());
		assert!(parse_payload(br#"{"v":1}"#).is_err());
	}

	/// Holding the pairing secret does not make a sender trusted: the peer's
	/// attachments are size-checked exactly as a local paste is.
	#[test]
	fn an_attachment_over_the_local_limit_is_refused() {
		let oversized = vec![0u8; ATTACHMENT_MAX_BYTES as usize + 1];
		let built = build_payload(&[note("x", vec![("big.bin", oversized)])]).unwrap();
		let err = parse_payload(&built).unwrap_err();
		assert_eq!(err.kind(), "invalid");
		assert!(err.message().contains("big.bin"), "{}", err.message());
	}

	#[test]
	fn a_note_over_the_per_note_attachment_limit_is_refused() {
		let attachments: Vec<(&str, Vec<u8>)> = (0..=ATTACHMENT_MAX_PER_NOTE)
			.map(|_| ("a.bin", vec![1u8]))
			.collect();
		let built = build_payload(&[note("x", attachments)]).unwrap();
		assert_eq!(parse_payload(&built).unwrap_err().kind(), "invalid");
	}

	/// The two inputs the *store* refuses deterministically. They have to be
	/// caught here, because a delivery failure stops the tick without counting
	/// towards the poison limit — so one of these left to `ops::add_note` would
	/// wedge the reader's mailbox for every later message, permanently.
	#[test]
	fn an_empty_body_or_a_zero_byte_attachment_is_refused() {
		let blank = br#"{"v":1,"notes":[{"body":"   \n ","attachments":[]}]}"#;
		let err = parse_payload(blank).unwrap_err();
		assert_eq!(err.kind(), "invalid");
		assert!(err.message().contains("empty body"), "{}", err.message());

		let hollow = build_payload(&[note("x", vec![("a.png", Vec::new())])]).unwrap();
		let err = parse_payload(&hollow).unwrap_err();
		assert_eq!(err.kind(), "invalid");
		assert!(err.message().contains("a.png"), "{}", err.message());
	}

	#[test]
	fn undecodable_attachment_bytes_are_refused() {
		let plaintext =
			br#"{"v":1,"notes":[{"body":"x","attachments":[{"name":"a.png","bytes":"!!!!"}]}]}"#;
		let err = parse_payload(plaintext).unwrap_err();
		assert_eq!(err.kind(), "invalid");
		assert!(err.message().contains("a.png"));
	}

	/// The Worker refuses anything shorter than this before it reaches KV, so the
	/// two numbers have to stay the same one.
	#[test]
	fn the_minimum_wire_length_is_a_nonce_plus_a_tag() {
		assert_eq!(MIN_WIRE_BYTES, 24 + 16);
	}
}
