//! The five commands the Settings view and the note context menu reach share
//! through.
//!
//! **No command reads a stored secret back.** `get_share_config` answers with
//! `tokenSet` and `secretSet` booleans, and the one exception is deliberate:
//! `generate_share_secret` returns the value it has just created, once, in the
//! reply that created it, because the user has to copy it to the other machine
//! and there is no second chance to show it.
//!
//! **All five are `#[tauri::command(async)]` over plain `fn` bodies**, and none
//! of them is an `async fn`. That attribute is Tauri's documented way to run a
//! blocking command off the main thread, so it buys the two that touch the
//! network their thread — a 20 MiB upload cannot freeze the panel — without
//! introducing an `async fn` or a `.await` anywhere in project code, which is
//! this feature's hard requirement. `ureq` has no runtime to conflict with.
//!
//! It is a departure from the rest of the IPC surface, where every wrapper is
//! `pub async fn`. `tests/commands.rs` parses both spellings for that reason.

use copper_core::attachments::{read_blob, ATTACHMENT_MAX_PER_NOTE};
use copper_core::store::error::StoreError;
use copper_core::store::{self, SharedStore};
use serde::{Deserialize, Serialize};
use tauri::State;

use super::config::{self, ShareConfig, ShareConfigPatch};
use super::protocol::{self, PayloadNote, MIN_WIRE_BYTES, SHARE_MAX_PAYLOAD_BYTES};
use super::relay::{self, HttpRelay, Relay, SendAck};

type Reply<T> = std::result::Result<T, StoreError>;

/// What **Test connection** found.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ShareTestOutcome {
	Ok,
	/// The relay answered, and refused the token. The one failure the user can
	/// fix from where they are standing, which is why it is not folded into
	/// `unreachable`.
	Unauthorised,
	Unconfigured {
		missing: String,
	},
	Unreachable {
		message: String,
	},
}

/// What a send turned out to be.
///
/// `unknown` is not a failure and not a success. A request whose outcome was
/// never learned may well have arrived, and telling the user it failed would
/// invite a resend that duplicates the note.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ShareSendOutcome {
	Sent {
		notes: usize,
	},
	/// The relay's 202: stored, but the head write failed, so nothing announces
	/// it yet. The next send announces it too.
	Delayed {
		notes: usize,
	},
	Unknown {
		message: String,
	},
	TooLarge {
		bytes: usize,
		limit: usize,
	},
	Unconfigured {
		missing: String,
	},
	Failed {
		message: String,
	},
}

/// The one-time reveal. The only reply in the whole surface carrying a secret.
///
/// **No derived `Debug`.** The one-time exception is about the *IPC reply*; it
/// does not extend to a log line, and a derived `Debug` on the one type in the
/// tree that holds a live pairing secret is one `{:?}` away from writing it
/// somewhere permanent.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedSecret {
	pub secret: String,
}

impl std::fmt::Debug for GeneratedSecret {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("GeneratedSecret { secret: <redacted> }")
	}
}

#[tauri::command(async)]
pub fn get_share_config() -> Reply<ShareConfig> {
	Ok(super::require_state()?.public())
}

/// Applies a patch and wakes the poller, so a change takes effect within a
/// second rather than within a minute.
#[tauri::command(async)]
pub fn set_share_config(patch: ShareConfigPatch) -> Reply<ShareConfig> {
	let state = super::require_state()?;
	let updated = state.mutate(|stored| {
		config::patch(stored, patch);
		// A configuration change makes any standing failure a claim about a setup
		// that no longer exists.
		stored.last_error = None;
		config::public(stored)
	})?;
	super::wake();
	Ok(updated)
}

/// Creates a pairing secret, stores it, and returns it **once**.
///
/// Both counters and `pending` reset with it: a new secret is a new pair of
/// mailboxes, so the old cursors describe something that no longer exists.
#[tauri::command(async)]
pub fn generate_share_secret() -> Reply<GeneratedSecret> {
	let state = super::require_state()?;
	let secret = super::crypto::generate_secret()?;

	let stored = secret.clone();
	state.mutate(move |config| {
		config.secret = stored;
		config.last_error = None;
		config::reset_counters(config);
	})?;
	super::wake();

	Ok(GeneratedSecret { secret })
}

/// Asks the relay for this device's own head pointer, and reports what happened.
///
/// The cheapest call in the protocol — one KV read — chosen precisely so that a
/// connection test costs the user nothing measurable.
#[tauri::command(async)]
pub fn share_test_relay() -> Reply<ShareTestOutcome> {
	let state = super::require_state()?;
	let (config, _) = state.snapshot();
	let ready = match config::resolve(&config) {
		Ok(ready) => ready,
		Err(missing) => return Ok(ShareTestOutcome::Unconfigured { missing: missing.0 }),
	};

	match HttpRelay::new(&ready.relay_url, &ready.token).head(&ready.own) {
		Ok(_) => Ok(ShareTestOutcome::Ok),
		Err(err) if relay::is_unauthorised(&err) => Ok(ShareTestOutcome::Unauthorised),
		Err(err) => Ok(ShareTestOutcome::Unreachable {
			message: err.message(),
		}),
	}
}

/// Encrypts the named notes and posts them to the other device's mailbox.
///
/// The order of the steps is the whole correctness argument.
///
/// 1. **Everything that can refuse happens before the network.** An empty
///    selection, an unconfigured share, a missing attachment blob, an over-cap
///    payload: all of them fail with nothing sent.
/// 2. **The sequence is reserved and persisted before the request leaves.** A
///    reserved sequence is never reused, whatever the network says. Without
///    that, a lost 204 would leave the counter unchanged and the *next* send
///    would overwrite a message the reader had not yet consumed — vanishing
///    while reporting success.
/// 3. **An outcome that was never learned is reported as `unknown`.** The note
///    may have arrived; sending it again would duplicate it, and the user is
///    told exactly that.
#[tauri::command(async)]
pub fn share_send_notes(ids: Vec<String>, state: State<'_, SharedStore>) -> Reply<ShareSendOutcome> {
	if ids.is_empty() {
		return Err(StoreError::Invalid("no notes were selected to send".into()));
	}

	let share = super::require_state()?;

	// **Taken before the configuration is read**, not after it. The guard is what
	// serialises sends in this process, and the sequence number is peeked below
	// and reserved further down — two calls that both read the counter before
	// either took the guard would peek the same value, seal two different notes
	// for the same slot, and the second would overwrite the first on the relay.
	// Everything from the peek to the reservation happens inside it.
	//
	// It also spaces sends out: KV allows about one write per second to a single
	// key and every send writes the head key.
	let _sending = share.send_guard();

	let (config, generation) = share.snapshot();
	// Checked separately from `resolve`, which answers "is this configuration
	// usable" and knows nothing about the switch. A send while the feature is off
	// must make no network call — a stale WebView holding a menu item open across
	// the toggle is exactly how one would otherwise get through.
	if !config.enabled {
		return Ok(ShareSendOutcome::Unconfigured {
			missing: "Share switch to on".into(),
		});
	}
	let ready = match config::resolve(&config) {
		Ok(ready) => ready,
		Err(missing) => return Ok(ShareSendOutcome::Unconfigured { missing: missing.0 }),
	};

	// The document and the path together, under one lock acquisition, and dropped
	// before anything slow: no store lock is ever held across a network call.
	let (space, space_path) = {
		let guard = store::lock(&state);
		(guard.active_space()?, guard.require_active_path()?)
	};

	// Read in **document order**, not in the order the ids arrived. The receiving
	// device writes them in the order it is given, so this is what makes a
	// multi-select arrive looking like what the sender sees.
	let selected: Vec<&copper_core::store::model::Note> = space
		.notes
		.iter()
		.filter(|note| ids.contains(&note.id))
		.collect();
	if selected.len() != ids.len() {
		return Err(StoreError::NotFound(
			"one of the selected notes is no longer in this space".into(),
		));
	}

	let mut notes = Vec::with_capacity(selected.len());
	// A running total, so an over-cap selection is refused without every blob
	// having been read into memory first. It counts base64 bytes, which is how
	// the attachments will travel.
	let mut accumulated = 0usize;
	for note in selected {
		if note.attachments.len() > ATTACHMENT_MAX_PER_NOTE {
			return Err(StoreError::Invalid(format!(
				"a note carries {} attachments, more than the {ATTACHMENT_MAX_PER_NOTE} a message \
				 can hold",
				note.attachments.len()
			)));
		}

		let mut attachments = Vec::with_capacity(note.attachments.len());
		for attachment in &note.attachments {
			// The whole send fails if a blob is missing, rather than shipping a note
			// with holes in it. A partial send reported as success is the one outcome
			// worse than a refused one.
			let bytes = read_blob(&space_path, &attachment.file)?;
			// 4 base64 characters per 3 bytes, rounded up.
			accumulated += bytes.len().div_ceil(3) * 4;
			if accumulated > SHARE_MAX_PAYLOAD_BYTES {
				return Ok(ShareSendOutcome::TooLarge {
					bytes: accumulated,
					limit: SHARE_MAX_PAYLOAD_BYTES,
				});
			}
			attachments.push((attachment.name.clone(), bytes));
		}

		accumulated += note.body.len();
		notes.push(PayloadNote {
			body: note.body.clone(),
			attachments,
		});
	}

	let count = notes.len();
	let plaintext = protocol::build_payload(&notes)?;
	// The blob bytes are in `plaintext` now, base64 and all. Holding the originals
	// as well would keep a second copy of a 20 MiB message alive across the seal
	// and the upload, for nothing.
	drop(notes);

	// **The exact finished size, before any network call at all.** XChaCha20-
	// Poly1305 is a stream cipher with a tag: the sealed message is the plaintext
	// plus a 24-byte nonce and a 16-byte tag, always, so the wire length is known
	// here without sealing anything. The running total above is a *memory* guard —
	// it stops a hopeless selection being read into memory blob by blob — and this
	// is the one that reports a real number.
	let wire_bytes = plaintext.len() + MIN_WIRE_BYTES;
	if wire_bytes > SHARE_MAX_PAYLOAD_BYTES {
		return Ok(ShareSendOutcome::TooLarge {
			bytes: wire_bytes,
			limit: SHARE_MAX_PAYLOAD_BYTES,
		});
	}

	let relay = HttpRelay::new(&ready.relay_url, &ready.token);

	// The sender's half of the re-sync rule, done here rather than in the poller.
	// A counter that is unknown — first run after enabling, or `share.json` was
	// lost — is recovered from the peer's head, so a send made before the first
	// poll cannot start again at zero and overwrite messages the other device has
	// not read yet. Doing it here also keeps an idle poll's cold cost at two reads
	// rather than three.
	//
	// It is not airtight, and cannot be with this protocol: KV is eventually
	// consistent, so a stale head can name a slot that is already occupied, and a
	// message stored by an earlier 202 is invisible to the head entirely. Recorded
	// in the task's Notes as the feature's known weakest edge.
	let seq = match config.next_seq {
		Some(seq) => seq,
		None => match relay
			.head(&ready.peer)
			.map_err(|err| StoreError::Unavailable(err.message()))?
		{
			Some(head) => super::advance(head)?,
			None => 0,
		},
	};
	// Sealed against the slot it will occupy, because `crypto::aad` binds the
	// sequence: a message moved to another slot does not open.
	let sealed = super::crypto::seal(&ready.keys, &ready.peer, seq, &plaintext)?;
	// Sealed, so the cleartext has no further reader. The upload below is the
	// longest stretch of the send, and the worst one to hold 20 MiB through.
	drop(plaintext);

	let next = super::advance(seq)?;
	// **Before the request leaves, and fenced.** The send guard keeps other sends
	// out of the window between the peek above and here, but it says nothing about
	// a *configuration* change: the user can replace the pairing secret or the
	// role while the payload is being built, and writing this counter
	// unconditionally would stamp a sequence belonging to the old pair of mailboxes
	// onto the new ones. A stale reservation stops here rather than posting.
	if share
		.apply_if_current(generation, |stored| stored.next_seq = Some(next))?
		.is_none()
	{
		return Ok(ShareSendOutcome::Failed {
			message: "the share settings changed while this note was being prepared; send it again"
				.into(),
		});
	}

	match relay.send(&ready.peer, seq, &sealed) {
		Ok(SendAck::Delivered) => Ok(ShareSendOutcome::Sent { notes: count }),
		Ok(SendAck::Unannounced) => Ok(ShareSendOutcome::Delayed { notes: count }),
		// A refusal is a *definite* answer — the relay replied, and replied that it
		// stored nothing. Reporting that as `unknown` would tell the user their note
		// might have arrived when it provably did not, which is the more expensive
		// mistake of the two: they would not send it again.
		Err(err) if relay::is_refusal(&err) => Ok(ShareSendOutcome::Failed {
			message: err.message(),
		}),
		// Everything else is ambiguous: the request had begun, so the message may be
		// stored even though the answer never arrived.
		Err(err) => Ok(ShareSendOutcome::Unknown {
			message: err.message(),
		}),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The frontend branches on `kind`, so every variant has to arrive as the
	/// same flat shape rather than as serde's default enum encoding.
	#[test]
	fn every_send_outcome_is_a_flat_kebab_case_kind() {
		for (outcome, kind) in [
			(ShareSendOutcome::Sent { notes: 2 }, "sent"),
			(ShareSendOutcome::Delayed { notes: 1 }, "delayed"),
			(
				ShareSendOutcome::Unknown {
					message: "no answer".into(),
				},
				"unknown",
			),
			(
				ShareSendOutcome::TooLarge {
					bytes: 1,
					limit: SHARE_MAX_PAYLOAD_BYTES,
				},
				"too-large",
			),
			(
				ShareSendOutcome::Unconfigured {
					missing: "relay URL".into(),
				},
				"unconfigured",
			),
			(
				ShareSendOutcome::Failed {
					message: "no".into(),
				},
				"failed",
			),
		] {
			let payload = serde_json::to_value(&outcome).unwrap();
			assert_eq!(payload["kind"], kind);
			assert_eq!(
				serde_json::from_value::<ShareSendOutcome>(payload).unwrap(),
				outcome,
				"{kind} does not round-trip"
			);
		}
	}

	#[test]
	fn every_test_outcome_is_a_flat_kebab_case_kind() {
		for (outcome, kind) in [
			(ShareTestOutcome::Ok, "ok"),
			(ShareTestOutcome::Unauthorised, "unauthorised"),
			(
				ShareTestOutcome::Unconfigured {
					missing: "relay token".into(),
				},
				"unconfigured",
			),
			(
				ShareTestOutcome::Unreachable {
					message: "no route".into(),
				},
				"unreachable",
			),
		] {
			let payload = serde_json::to_value(&outcome).unwrap();
			assert_eq!(payload["kind"], kind);
			assert_eq!(serde_json::from_value::<ShareTestOutcome>(payload).unwrap(), outcome);
		}
	}

	#[test]
	fn the_generated_secret_reply_carries_exactly_one_field() {
		let payload = serde_json::to_value(GeneratedSecret {
			secret: "abc".into(),
		})
		.unwrap();
		assert_eq!(payload["secret"], "abc");
		assert_eq!(payload.as_object().unwrap().len(), 1);
	}
}
