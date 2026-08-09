//! Key derivation and the sealed-message format. Pure functions, no I/O.
//!
//! Everything the relay is trusted with is decided here. The Worker, Cloudflare
//! and anyone reading the KV namespace see a nonce and a ciphertext; the key
//! that opens it is derived on the two devices from a pairing secret that never
//! crosses a network in any form.
//!
//! ```text
//! HKDF-SHA256, salt = b"copper-share-v1", ikm = the 32 secret bytes
//!
//!   info = "copper-share/mailbox/1" -> 16 bytes -> lowercase hex -> mailbox_1
//!   info = "copper-share/mailbox/2" -> 16 bytes -> lowercase hex -> mailbox_2
//!   info = "copper-share/enc"       -> 32 bytes ->                  enc_key
//! ```
//!
//! **No password stretching, deliberately.** HKDF is the right derivation for a
//! high-entropy input and the wrong one for a memorable phrase. The pairing
//! secret is exactly 32 bytes from the OS CSPRNG, and [`decode_secret`] refuses
//! anything that is not, so the assumption cannot be quietly violated by a
//! hand-typed value. A memorable-passphrase mode would need Argon2id in front of
//! this; it is recorded as an open question and not built.

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use copper_core::store::error::{Result, StoreError};
use hkdf::Hkdf;
use sha2::Sha256;

use super::protocol::MIN_WIRE_BYTES;

/// The domain separator, in the HKDF salt and again in every AAD.
const DOMAIN: &[u8] = b"copper-share-v1";

const NONCE_BYTES: usize = 24;

/// The encoding the pairing secret is copied between machines in. URL-safe and
/// unpadded, so the value survives being pasted anywhere without an escape.
const SECRET_ENCODING: base64::engine::general_purpose::GeneralPurpose =
	base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// Everything one pairing secret derives.
///
/// **No `Debug`, derived or otherwise.** `enc_key` is the whole confidentiality
/// of the feature, and a derived `Debug` is one `{:?}` in one error message away
/// from putting it in a log file. The two mailbox identifiers are not secret —
/// they travel in every request — but they live here, so the type as a whole
/// does not get printed.
pub struct Keys {
	pub mailbox_1: String,
	pub mailbox_2: String,
	pub enc_key: [u8; 32],
}

/// Derives both mailbox identifiers and the encryption key.
///
/// Infallible: HKDF-SHA256 `expand` can only fail for an output longer than 255
/// hash blocks, and these three are 16, 16 and 32 bytes.
pub fn derive(secret: &[u8; 32]) -> Keys {
	let hkdf = Hkdf::<Sha256>::new(Some(DOMAIN), secret);

	let mut mailbox_1 = [0u8; 16];
	let mut mailbox_2 = [0u8; 16];
	let mut enc_key = [0u8; 32];
	// `expect` rather than a propagated error: see above. A panic here would mean
	// the constant lengths on the previous three lines had been changed to
	// something over 8160 bytes.
	hkdf.expand(b"copper-share/mailbox/1", &mut mailbox_1)
		.expect("a 16-byte HKDF expansion cannot be too long");
	hkdf.expand(b"copper-share/mailbox/2", &mut mailbox_2)
		.expect("a 16-byte HKDF expansion cannot be too long");
	hkdf.expand(b"copper-share/enc", &mut enc_key)
		.expect("a 32-byte HKDF expansion cannot be too long");

	Keys {
		mailbox_1: hex(&mailbox_1),
		mailbox_2: hex(&mailbox_2),
		enc_key,
	}
}

fn hex(bytes: &[u8]) -> String {
	let mut out = String::with_capacity(bytes.len() * 2);
	for byte in bytes {
		out.push_str(&format!("{byte:02x}"));
	}
	out
}

/// What a ciphertext is bound to, besides the key.
///
/// The mailbox and the sequence number are authenticated but not encrypted, so a
/// party holding the relay token cannot move a message to another slot or the
/// other mailbox and have it open. That binding is also what lets **one**
/// encryption key serve both directions safely.
///
/// It does not prevent replay into the *same* slot, and nothing cryptographic
/// could — the ciphertext is valid there by construction. Replay protection is
/// the reader's cursor, which only ever advances.
pub fn aad(mailbox: &str, seq: u64) -> Vec<u8> {
	let mut out = Vec::with_capacity(DOMAIN.len() + mailbox.len() + 8);
	out.extend_from_slice(DOMAIN);
	out.extend_from_slice(mailbox.as_bytes());
	out.extend_from_slice(&seq.to_be_bytes());
	out
}

/// `nonce (24 bytes) || XChaCha20-Poly1305(enc_key, nonce, plaintext, aad)`.
///
/// The nonce is 24 random bytes per message. A 192-bit random nonce has a
/// collision bound around 2^96 messages, which is why no send budget has to be
/// reasoned about — AES-GCM's 96-bit nonce would have needed one.
pub fn seal(keys: &Keys, mailbox: &str, seq: u64, plaintext: &[u8]) -> Result<Vec<u8>> {
	let mut nonce = [0u8; NONCE_BYTES];
	getrandom::fill(&mut nonce)
		.map_err(|err| StoreError::Io(format!("could not draw a random nonce: {err}")))?;

	let cipher = cipher(keys)?;
	let sealed = cipher
		.encrypt(
			&XNonce::from(nonce),
			Payload {
				msg: plaintext,
				aad: &aad(mailbox, seq),
			},
		)
		.map_err(|_| StoreError::Io("this note could not be encrypted".into()))?;

	let mut wire = Vec::with_capacity(NONCE_BYTES + sealed.len());
	wire.extend_from_slice(&nonce);
	wire.extend_from_slice(&sealed);
	Ok(wire)
}

/// The inverse of [`seal`], for the exact mailbox and sequence it was sealed to.
///
/// Every failure is the same `invalid`, and its message quotes no bytes and no
/// key material. A wrong key, a flipped bit, a message moved to another slot and
/// a truncated body are indistinguishable to a reader by design: an attacker who
/// can tell them apart learns which of their guesses was closer.
pub fn open(keys: &Keys, mailbox: &str, seq: u64, wire: &[u8]) -> Result<Vec<u8>> {
	// Before the split below, which would otherwise panic on a short slice. A
	// relay is untrusted input like any other.
	if wire.len() < MIN_WIRE_BYTES {
		return Err(opaque_failure());
	}
	let (nonce, sealed) = wire.split_at(NONCE_BYTES);
	let nonce = XNonce::try_from(nonce).map_err(|_| opaque_failure())?;

	cipher(keys)?
		.decrypt(
			&nonce,
			Payload {
				msg: sealed,
				aad: &aad(mailbox, seq),
			},
		)
		.map_err(|_| opaque_failure())
}

fn opaque_failure() -> StoreError {
	StoreError::Invalid("this message could not be decrypted with the current pairing secret".into())
}

fn cipher(keys: &Keys) -> Result<XChaCha20Poly1305> {
	XChaCha20Poly1305::new_from_slice(&keys.enc_key)
		.map_err(|_| StoreError::Invalid("the derived encryption key is the wrong size".into()))
}

/// A new pairing secret: 32 bytes from the OS CSPRNG, URL-safe base64.
///
/// The one value in this feature the user ever sees. It is returned once, by the
/// command that creates it, so it can be copied to the other machine — and never
/// read back afterwards.
pub fn generate_secret() -> Result<String> {
	let mut secret = [0u8; 32];
	getrandom::fill(&mut secret)
		.map_err(|err| StoreError::Io(format!("could not draw a pairing secret: {err}")))?;
	Ok(SECRET_ENCODING.encode(secret))
}

/// Parses a pairing secret, refusing anything that is not exactly 32 bytes.
///
/// The refusal is the point rather than a formality. This design does no
/// password stretching because the secret is full-entropy; accepting a
/// hand-typed short value would silently turn HKDF into a key derivation over
/// something guessable.
pub fn decode_secret(text: &str) -> Result<[u8; 32]> {
	let bytes = SECRET_ENCODING
		.decode(text.trim())
		.map_err(|_| bad_secret())?;
	bytes.try_into().map_err(|_| bad_secret())
}

fn bad_secret() -> StoreError {
	StoreError::Invalid(
		"a pairing secret must be the exact value Generate produced on the other device".into(),
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn secret(fill: u8) -> [u8; 32] {
		[fill; 32]
	}

	#[test]
	fn derivation_is_deterministic_per_secret_and_differs_across_secrets() {
		let a = derive(&secret(1));
		let again = derive(&secret(1));
		let b = derive(&secret(2));

		assert_eq!(a.mailbox_1, again.mailbox_1);
		assert_eq!(a.mailbox_2, again.mailbox_2);
		assert_eq!(a.enc_key, again.enc_key);

		assert_ne!(a.mailbox_1, b.mailbox_1);
		assert_ne!(a.mailbox_2, b.mailbox_2);
		assert_ne!(a.enc_key, b.enc_key);
	}

	/// The two mailboxes are what make the protocol one-way per key. If they
	/// collided, each device would read what it had just written.
	#[test]
	fn the_two_mailboxes_differ_and_match_the_workers_pattern() {
		let keys = derive(&secret(7));
		assert_ne!(keys.mailbox_1, keys.mailbox_2);
		for mailbox in [&keys.mailbox_1, &keys.mailbox_2] {
			assert_eq!(mailbox.len(), 32, "{mailbox} is not 32 hex characters");
			assert!(
				mailbox.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
				"{mailbox} is not lowercase hex, which the Worker rejects with 400"
			);
		}
	}

	#[test]
	fn a_seal_open_round_trip_returns_the_input() {
		let keys = derive(&secret(3));
		let plaintext = b"the body of a note, verbatim";
		let wire = seal(&keys, &keys.mailbox_1, 42, plaintext).unwrap();

		assert!(wire.len() >= MIN_WIRE_BYTES);
		assert_eq!(open(&keys, &keys.mailbox_1, 42, &wire).unwrap(), plaintext);
	}

	/// Every byte position, not one arbitrary index: a tamper test that checks a
	/// single offset proves nothing about the nonce, the tag or the body
	/// separately.
	#[test]
	fn flipping_any_byte_makes_open_fail() {
		let keys = derive(&secret(4));
		let wire = seal(&keys, &keys.mailbox_2, 0, b"tamper me").unwrap();

		for index in 0..wire.len() {
			let mut damaged = wire.clone();
			damaged[index] ^= 0x01;
			assert!(
				open(&keys, &keys.mailbox_2, 0, &damaged).is_err(),
				"byte {index} could be flipped without failing the open"
			);
		}
	}

	#[test]
	fn a_message_moved_to_another_slot_or_mailbox_does_not_open() {
		let keys = derive(&secret(5));
		let wire = seal(&keys, &keys.mailbox_1, 9, b"bound").unwrap();

		assert!(open(&keys, &keys.mailbox_1, 10, &wire).is_err(), "the sequence is not bound");
		assert!(open(&keys, &keys.mailbox_2, 9, &wire).is_err(), "the mailbox is not bound");
		assert!(open(&derive(&secret(6)), &keys.mailbox_1, 9, &wire).is_err());
	}

	#[test]
	fn two_seals_of_identical_input_differ() {
		let keys = derive(&secret(8));
		let first = seal(&keys, &keys.mailbox_1, 1, b"same").unwrap();
		let second = seal(&keys, &keys.mailbox_1, 1, b"same").unwrap();
		assert_ne!(first, second, "the nonce is not being drawn per message");
	}

	#[test]
	fn a_wire_message_shorter_than_the_minimum_is_refused_rather_than_panicking() {
		let keys = derive(&secret(9));
		for length in [0, 1, MIN_WIRE_BYTES - 1] {
			let short = vec![0u8; length];
			assert!(open(&keys, &keys.mailbox_1, 0, &short).is_err());
		}
	}

	#[test]
	fn a_generated_secret_round_trips_and_the_wrong_length_is_refused() {
		let secret = generate_secret().unwrap();
		assert_eq!(decode_secret(&secret).unwrap().len(), 32);
		assert_ne!(secret, generate_secret().unwrap());

		let short = SECRET_ENCODING.encode([0u8; 16]);
		let long = SECRET_ENCODING.encode([0u8; 33]);
		assert!(decode_secret(&short).is_err(), "a 16-byte secret was accepted");
		assert!(decode_secret(&long).is_err(), "a 33-byte secret was accepted");
		assert!(decode_secret("not base64 at all !!").is_err());
		assert!(decode_secret("").is_err());
	}

	/// Whitespace either side is stripped, because the value reaches the second
	/// machine through a clipboard.
	#[test]
	fn a_pasted_secret_survives_surrounding_whitespace() {
		let secret = generate_secret().unwrap();
		let pasted = format!("  {secret}\r\n");
		assert_eq!(decode_secret(&pasted).unwrap(), decode_secret(&secret).unwrap());
	}

	#[test]
	fn the_associated_data_carries_the_domain_the_mailbox_and_the_sequence() {
		let data = aad("0123456789abcdef0123456789abcdef", 1);
		assert!(data.starts_with(DOMAIN));
		assert_eq!(data.len(), DOMAIN.len() + 32 + 8);
		assert_ne!(data, aad("0123456789abcdef0123456789abcdef", 2));
	}
}
