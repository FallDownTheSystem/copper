//! `share.json` — the shapes in it, and the one function that decides whether
//! the feature is usable.
//!
//! **Why this is not in `settings.json`.** Every field of
//! `copper_core::store::settings::Settings` is `pub` and is serialised to the
//! WebView by `get_settings`. `#[serde(skip_serializing)]` cannot separate the
//! two: the same `Serialize` impl writes the file, so skipping a field would
//! stop it being persisted at all. A pairing secret in `Settings` would
//! therefore either reach the frontend on every settings pull or not survive a
//! restart. It lives in its own file instead, with `copper-cli`'s
//! `cli-state.json` as the precedent for a module owning a small state file of
//! its own.
//!
//! This is IPC isolation, not encryption at rest. Anything running as the user
//! can read `share.json`, and that is explicitly outside the threat model —
//! exactly as it is for the `.copper` documents themselves.
//!
//! ```json
//! {
//!   "enabled": false, "relayUrl": "", "role": "first",
//!   "token": "", "secret": "",
//!   "nextSeq": null, "nextRead": null, "pending": null, "lastError": null
//! }
//! ```

use std::fmt;
use std::path::{Path, PathBuf};

use copper_core::store::atomic;
use copper_core::store::error::Result;
use copper_core::store::format::to_git_json;
use serde::{Deserialize, Serialize};

use super::crypto::{self, Keys};

pub const FILE_NAME: &str = "share.json";

/// Which mailbox this device reads and which it writes.
///
/// If both machines are set to the same role, each writes to a mailbox neither
/// reads and reads a mailbox neither writes: **nothing is delivered, in either
/// direction**. Nothing can detect that from one side, so the settings row says
/// it in its description. The failure is inert rather than destructive — the
/// notes simply stay on the machine that sent them.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ShareRole {
	#[default]
	First,
	Second,
}

/// A message the reader has seen a head pointer for but has not consumed.
///
/// One slot is enough because the drain is strictly sequential: only the head of
/// the line can ever be stuck.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
	pub seq: u64,
	/// RFC 3339, the same spelling every timestamp in a `.copper` document uses.
	pub first_miss_at: String,
	pub failures: u32,
}

/// Everything in `share.json`, secrets included. Never crosses IPC.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct StoredConfig {
	pub enabled: bool,
	pub relay_url: String,
	pub role: ShareRole,
	pub token: String,
	pub secret: String,
	/// The next sequence a send will claim. `None` means "ask the relay".
	pub next_seq: Option<u64>,
	/// The next sequence the reader will fetch. `None` means "ask the relay".
	pub next_read: Option<u64>,
	pub pending: Option<Pending>,
	pub last_error: Option<String>,
}

/// Hand-written, so that no `{:?}` anywhere — a log line, an error message, a
/// panic payload — can print the two values this whole feature depends on
/// staying private.
impl fmt::Debug for StoredConfig {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		fn shown(value: &str) -> &'static str {
			if value.is_empty() {
				"<unset>"
			} else {
				"<set>"
			}
		}

		f.debug_struct("StoredConfig")
			.field("enabled", &self.enabled)
			.field("relay_url", &self.relay_url)
			.field("role", &self.role)
			.field("token", &shown(&self.token))
			.field("secret", &shown(&self.secret))
			.field("next_seq", &self.next_seq)
			.field("next_read", &self.next_read)
			.field("pending", &self.pending)
			.field("last_error", &self.last_error)
			.finish()
	}
}

/// The share configuration as the frontend is allowed to see it.
///
/// The two secrets are reduced to booleans here and nowhere else, so there is
/// exactly one place that could ever leak them and it is this struct's
/// definition. No command returns a stored secret — the single exception is
/// `generate_share_secret`, which hands back the value it has just created, once,
/// so the user can copy it to the other machine.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareConfig {
	pub enabled: bool,
	pub relay_url: String,
	pub role: ShareRole,
	pub token_set: bool,
	pub secret_set: bool,
	/// Whether [`resolve`] accepts this configuration.
	///
	/// Carried so the frontend does not have to write a second, weaker copy of
	/// the same rules to decide whether **Send to my other device** is available.
	/// It is not derivable from the three fields above: a relay URL can be
	/// present and refused, and a pairing secret can be stored and be the wrong
	/// length. Rust is the one authority on "is this usable", and this is how it
	/// says so.
	pub configured: bool,
	pub last_error: Option<String>,
}

/// A partial update. Every field is optional, and the two secrets are
/// three-state: absent leaves the stored value, a string replaces it, `null`
/// clears it — the same rule the panel position already uses.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ShareConfigPatch {
	pub enabled: Option<bool>,
	pub relay_url: Option<String>,
	pub role: Option<ShareRole>,
	#[serde(default, deserialize_with = "three_state")]
	pub token: Option<Option<String>>,
	#[serde(default, deserialize_with = "three_state")]
	pub secret: Option<Option<String>>,
}

/// Redacted for the same reason [`StoredConfig`]'s is, and it is the *more*
/// exposed of the two: a patch is the value in flight, so it is what an IPC
/// trace, a `dbg!` left in a command wrapper or an `assert_eq!` failure in a
/// test would print. The three-state fields keep their shape — `absent`,
/// `clear` and `set` are what a reader needs — and lose only the value.
impl fmt::Debug for ShareConfigPatch {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		fn shown(field: &Option<Option<String>>) -> &'static str {
			match field {
				None => "<absent>",
				Some(None) => "<clear>",
				Some(Some(_)) => "<set>",
			}
		}

		f.debug_struct("ShareConfigPatch")
			.field("enabled", &self.enabled)
			.field("relay_url", &self.relay_url)
			.field("role", &self.role)
			.field("token", &shown(&self.token))
			.field("secret", &shown(&self.secret))
			.finish()
	}
}

/// Distinguishes "the key was absent" from "the key was `null`".
///
/// serde collapses both into `None` for a plain `Option<String>`, and the whole
/// point of the secret fields is that those two mean opposite things: leave the
/// stored secret alone, versus clear it. `Option<Option<String>>` with
/// `#[serde(default)]` is the shape that keeps them apart.
fn three_state<'de, D>(deserializer: D) -> std::result::Result<Option<Option<String>>, D::Error>
where
	D: serde::Deserializer<'de>,
{
	Option::<String>::deserialize(deserializer).map(Some)
}

/// A resolved, usable configuration. Everything the network paths need.
///
/// **No derived `Debug`**: it holds [`Keys`] and the bearer token.
pub struct Ready {
	pub keys: Keys,
	/// The mailbox this device reads.
	pub own: String,
	/// The mailbox this device writes to.
	pub peer: String,
	pub relay_url: String,
	pub token: String,
}

// --- load and save -----------------------------------------------------------

/// `share.json` beside `settings.json`.
pub fn path(dir: &Path) -> PathBuf {
	dir.join(FILE_NAME)
}

/// The stored configuration, or defaults.
///
/// A missing, unreadable or unparseable file loads as defaults and **nothing is
/// set aside**, which is the opposite of what `settings.json` does with a
/// corrupt file. The reason is what is at stake: this file holds a URL, two
/// values the user can paste again from the other machine, and two counters that
/// re-sync from the relay by design. `settings.json`'s set-aside dance buys
/// recovery of things that cannot be retyped; here it would only buy failure
/// modes.
///
/// The directory is a parameter rather than a `default_config_dir()` call inside
/// this function, so the unit tests below can point it at a tempdir.
pub fn load(dir: &Path) -> StoredConfig {
	std::fs::read_to_string(path(dir))
		.ok()
		.and_then(|text| serde_json::from_str(&text).ok())
		.unwrap_or_default()
}

/// Writes the configuration through the store's temp-file-plus-rename discipline.
///
/// `atomic::write_atomic` is documented as single-writer only, which is exactly
/// why `state::ShareState` exists: it is the one in-process owner, and this
/// function is reached only through it.
pub fn save(dir: &Path, config: &StoredConfig) -> Result<()> {
	std::fs::create_dir_all(dir)
		.map_err(|err| copper_core::store::error::io_err(dir, "create", &err))?;
	// `to_git_json` rather than a `to_string_pretty` of its own: that function is
	// the workspace's one declaration of what a JSON file Copper writes looks
	// like, and `copper-cli`'s `cli-state.json` — the precedent this module cites —
	// is written through it too.
	atomic::write_atomic(&path(dir), &to_git_json(config)?)
}

// --- patching ----------------------------------------------------------------

/// Applies a patch in place.
///
/// Two rules here are not obvious from the shapes.
///
/// **Changing the relay URL clears the stored token.** The token is a bearer
/// credential. A frontend that can point the relay at a new host must not be
/// able to make Rust hand the old host's credential to it.
///
/// **Changing the relay URL, the role or the secret resets both counters and
/// `pending`.** All three change which mailboxes the counters describe, so
/// keeping them would mean a cursor into a mailbox it was never counting.
pub fn patch(config: &mut StoredConfig, patch: ShareConfigPatch) {
	let mut mailboxes_changed = false;

	if let Some(enabled) = patch.enabled {
		config.enabled = enabled;
	}
	if let Some(url) = patch.relay_url {
		let url = url.trim().to_string();
		if url != config.relay_url {
			config.relay_url = url;
			config.token = String::new();
			mailboxes_changed = true;
		}
	}
	if let Some(role) = patch.role {
		if role != config.role {
			config.role = role;
			mailboxes_changed = true;
		}
	}
	// After the URL, so that setting both in one patch stores the token rather
	// than having it cleared by the URL change that arrived beside it.
	if let Some(token) = patch.token {
		config.token = token.unwrap_or_default().trim().to_string();
	}
	if let Some(secret) = patch.secret {
		let secret = secret.unwrap_or_default().trim().to_string();
		if secret != config.secret {
			config.secret = secret;
			mailboxes_changed = true;
		}
	}

	if mailboxes_changed {
		reset_counters(config);
	}
}

/// Forgets where both cursors had got to, so they re-sync from the relay.
pub fn reset_counters(config: &mut StoredConfig) {
	config.next_seq = None;
	config.next_read = None;
	config.pending = None;
}

/// The only way a configuration crosses the IPC boundary.
pub fn public(config: &StoredConfig) -> ShareConfig {
	ShareConfig {
		enabled: config.enabled,
		relay_url: config.relay_url.clone(),
		role: config.role,
		token_set: !config.token.is_empty(),
		secret_set: !config.secret.is_empty(),
		configured: resolve(config).is_ok(),
		last_error: config.last_error.clone(),
	}
}

// --- resolving ---------------------------------------------------------------

/// The name of the first missing or invalid field.
///
/// A newtype rather than a bare `String` so a caller cannot accidentally
/// interpolate a resolve failure where an error message belongs; the frontend
/// gets it as `{ kind: 'unconfigured', missing }` and writes its own sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing(pub String);

/// Everything the network paths need, or the name of what is missing.
///
/// Every command and the poller resolve through this one function, so "what is
/// missing" has exactly one spelling and one priority order.
///
/// The URL rules are enforced here rather than at the input: `https` only, no
/// credentials, no query, no fragment. The client also follows no redirects, so
/// an `https` URL cannot be bounced to an `http` one after the check.
pub fn resolve(config: &StoredConfig) -> std::result::Result<Ready, Missing> {
	let relay_url = validate_url(&config.relay_url)?;

	if config.token.is_empty() {
		return Err(Missing("relay token".into()));
	}

	let secret = crypto::decode_secret(&config.secret).map_err(|_| Missing("pairing secret".into()))?;
	let keys = crypto::derive(&secret);

	// First reads mailbox 1 and writes to mailbox 2; Second is the mirror image.
	// The two mailboxes give every key exactly one writer and one reader, which is
	// what removes any need for compare-and-swap under KV's eventual consistency.
	let (own, peer) = match config.role {
		ShareRole::First => (keys.mailbox_1.clone(), keys.mailbox_2.clone()),
		ShareRole::Second => (keys.mailbox_2.clone(), keys.mailbox_1.clone()),
	};

	Ok(Ready {
		keys,
		own,
		peer,
		relay_url,
		token: config.token.clone(),
	})
}

/// The relay URL, with its trailing slash removed, or the reason it is unusable.
fn validate_url(raw: &str) -> std::result::Result<String, Missing> {
	let url = raw.trim().trim_end_matches('/');
	let missing = || Missing("relay URL".into());

	let rest = url.strip_prefix("https://").ok_or_else(missing)?;
	if rest.is_empty() {
		return Err(missing());
	}
	// No credentials, no query, no fragment. A URL carrying any of them is either
	// a mistake or an attempt to make the client send the bearer token somewhere
	// it does not expect, and neither is worth guessing about.
	if rest.contains('@') || rest.contains('?') || rest.contains('#') {
		return Err(missing());
	}
	// The host part has to look like a host: no whitespace, no backslashes.
	let host = rest.split('/').next().unwrap_or_default();
	if host.is_empty() || host.contains(char::is_whitespace) || host.contains('\\') {
		return Err(missing());
	}

	Ok(url.to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn configured() -> StoredConfig {
		StoredConfig {
			enabled: true,
			relay_url: "https://copper-relay.example.workers.dev".into(),
			role: ShareRole::First,
			token: "a-long-random-token".into(),
			secret: crypto::generate_secret().unwrap(),
			next_seq: Some(4),
			next_read: Some(7),
			pending: None,
			last_error: None,
		}
	}

	#[test]
	fn an_absent_file_loads_as_defaults() {
		let dir = tempfile::tempdir().unwrap();
		let config = load(dir.path());

		assert_eq!(config, StoredConfig::default());
		assert!(!config.enabled, "the feature must be off until the user turns it on");
		assert_eq!(config.role, ShareRole::First);
	}

	#[test]
	fn a_corrupt_file_loads_as_defaults_and_is_not_set_aside() {
		let dir = tempfile::tempdir().unwrap();
		std::fs::write(path(dir.path()), "{ this is not json").unwrap();

		assert_eq!(load(dir.path()), StoredConfig::default());
		assert!(path(dir.path()).exists(), "a corrupt file is left alone, not moved");
	}

	#[test]
	fn a_save_load_round_trip_preserves_the_counters_and_pending() {
		let dir = tempfile::tempdir().unwrap();
		let mut config = configured();
		config.pending = Some(Pending {
			seq: 12,
			first_miss_at: "2026-08-09T16:00:00Z".into(),
			failures: 2,
		});
		config.last_error = Some("something went wrong".into());

		save(dir.path(), &config).unwrap();
		assert_eq!(load(dir.path()), config);
	}

	/// A file written by an older build has none of the newer keys, and must load
	/// rather than reset the user's whole configuration.
	#[test]
	fn a_partial_file_fills_the_missing_keys_with_defaults() {
		let dir = tempfile::tempdir().unwrap();
		std::fs::write(path(dir.path()), r#"{"enabled":true,"relayUrl":"https://x.dev"}"#).unwrap();

		let config = load(dir.path());
		assert!(config.enabled);
		assert_eq!(config.relay_url, "https://x.dev");
		assert_eq!(config.next_seq, None);
		assert_eq!(config.role, ShareRole::First);
	}

	#[test]
	fn the_secrets_are_three_state() {
		let mut config = configured();

		// Absent leaves them.
		patch(&mut config, ShareConfigPatch::default());
		assert_eq!(config.token, "a-long-random-token");
		assert!(!config.secret.is_empty());

		// A string replaces.
		patch(
			&mut config,
			ShareConfigPatch {
				token: Some(Some("replaced".into())),
				..Default::default()
			},
		);
		assert_eq!(config.token, "replaced");

		// Null clears.
		patch(
			&mut config,
			ShareConfigPatch {
				token: Some(None),
				..Default::default()
			},
		);
		assert!(config.token.is_empty());
	}

	/// serde has to keep "absent" and "null" apart, or the clear and the leave
	/// become the same request.
	#[test]
	fn absent_and_null_deserialise_differently() {
		let absent: ShareConfigPatch = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
		assert_eq!(absent.token, None, "an absent key must not read as a clear");

		let cleared: ShareConfigPatch = serde_json::from_str(r#"{"token":null}"#).unwrap();
		assert_eq!(cleared.token, Some(None), "a null must read as a clear");

		let set: ShareConfigPatch = serde_json::from_str(r#"{"token":"x"}"#).unwrap();
		assert_eq!(set.token, Some(Some("x".into())));
	}

	#[test]
	fn changing_the_relay_url_clears_the_token_and_resets_the_counters() {
		let mut config = configured();
		patch(
			&mut config,
			ShareConfigPatch {
				relay_url: Some("https://other.workers.dev".into()),
				..Default::default()
			},
		);

		assert!(config.token.is_empty(), "the old host's token followed the URL");
		assert_eq!(config.next_seq, None);
		assert_eq!(config.next_read, None);
	}

	/// Setting both in one patch has to store the token, not have it cleared by
	/// the URL that arrived beside it — which is what the Settings view does when
	/// the user fills in a fresh install.
	#[test]
	fn a_url_and_a_token_in_one_patch_keep_the_token() {
		let mut config = StoredConfig::default();
		patch(
			&mut config,
			ShareConfigPatch {
				relay_url: Some("https://relay.workers.dev".into()),
				token: Some(Some("fresh".into())),
				..Default::default()
			},
		);

		assert_eq!(config.relay_url, "https://relay.workers.dev");
		assert_eq!(config.token, "fresh");
	}

	#[test]
	fn re_setting_the_same_url_keeps_the_token_and_the_counters() {
		let mut config = configured();
		let same = config.relay_url.clone();
		patch(
			&mut config,
			ShareConfigPatch {
				relay_url: Some(same),
				..Default::default()
			},
		);

		assert_eq!(config.token, "a-long-random-token");
		assert_eq!(config.next_seq, Some(4));
	}

	#[test]
	fn changing_the_role_or_the_secret_resets_the_counters() {
		for patched in [
			ShareConfigPatch {
				role: Some(ShareRole::Second),
				..Default::default()
			},
			ShareConfigPatch {
				secret: Some(Some(crypto::generate_secret().unwrap())),
				..Default::default()
			},
		] {
			let mut config = configured();
			patch(&mut config, patched);
			assert_eq!(config.next_seq, None);
			assert_eq!(config.next_read, None);
			assert_eq!(config.pending, None);
		}
	}

	#[test]
	fn merely_toggling_enabled_keeps_the_counters() {
		let mut config = configured();
		patch(
			&mut config,
			ShareConfigPatch {
				enabled: Some(false),
				..Default::default()
			},
		);

		assert!(!config.enabled);
		assert_eq!(config.next_seq, Some(4), "a toggle must not lose the sender's place");
		assert_eq!(config.next_read, Some(7));
	}

	#[test]
	fn the_public_shape_carries_no_secret_value() {
		let config = configured();
		let text = serde_json::to_string(&public(&config)).unwrap();

		assert!(!text.contains(&config.token), "the relay token crossed IPC: {text}");
		assert!(!text.contains(&config.secret), "the pairing secret crossed IPC: {text}");
		assert!(text.contains(r#""tokenSet":true"#), "{text}");
		assert!(text.contains(r#""secretSet":true"#), "{text}");
		assert!(!text.contains("\"token\""), "{text}");
		assert!(!text.contains("\"secret\""), "{text}");
	}

	#[test]
	fn an_unset_secret_reads_as_not_set() {
		let public = public(&StoredConfig::default());
		assert!(!public.token_set);
		assert!(!public.secret_set);
	}

	/// A `{:?}` in a log line is the realistic way a secret escapes, so the
	/// `Debug` impl is asserted rather than trusted.
	#[test]
	fn debug_prints_neither_secret() {
		let config = configured();
		let printed = format!("{config:?}");

		assert!(!printed.contains(&config.token), "{printed}");
		assert!(!printed.contains(&config.secret), "{printed}");
		assert!(printed.contains("<set>"), "{printed}");
		assert!(format!("{:?}", StoredConfig::default()).contains("<unset>"));
	}

	#[test]
	fn resolve_names_the_missing_field_in_priority_order() {
		let mut config = StoredConfig::default();
		assert_eq!(resolve(&config).err(), Some(Missing("relay URL".into())));

		config.relay_url = "https://relay.workers.dev".into();
		assert_eq!(resolve(&config).err(), Some(Missing("relay token".into())));

		config.token = "t".into();
		assert_eq!(resolve(&config).err(), Some(Missing("pairing secret".into())));

		config.secret = "not a 32-byte value".into();
		assert_eq!(resolve(&config).err(), Some(Missing("pairing secret".into())));

		config.secret = crypto::generate_secret().unwrap();
		assert!(resolve(&config).is_ok());
	}

	#[test]
	fn resolve_refuses_a_url_that_is_not_plain_https() {
		let mut config = configured();
		for bad in [
			"http://relay.workers.dev",
			"ftp://relay.workers.dev",
			"relay.workers.dev",
			"https://",
			"https://user:pass@relay.workers.dev",
			"https://relay.workers.dev?token=x",
			"https://relay.workers.dev#fragment",
			"https://relay .workers.dev",
			"",
			"   ",
		] {
			config.relay_url = bad.into();
			assert_eq!(
				resolve(&config).err(),
				Some(Missing("relay URL".into())),
				"{bad} was accepted as a relay URL"
			);
		}
	}

	#[test]
	fn resolve_trims_a_trailing_slash_so_paths_are_joined_once() {
		let mut config = configured();
		config.relay_url = "https://relay.workers.dev/".into();
		assert_eq!(resolve(&config).unwrap().relay_url, "https://relay.workers.dev");
	}

	/// The whole protocol rests on this: the two devices must not read the same
	/// mailbox, and each must write the one the other reads.
	#[test]
	fn the_two_roles_are_mirror_images() {
		let mut config = configured();
		let first = resolve(&config).unwrap();

		config.role = ShareRole::Second;
		let second = resolve(&config).unwrap();

		assert_eq!(first.own, second.peer);
		assert_eq!(first.peer, second.own);
		assert_ne!(first.own, first.peer);
		assert_eq!(first.keys.enc_key, second.keys.enc_key, "one key serves both directions");
	}

	#[test]
	fn a_role_round_trips_through_json_as_a_lowercase_word() {
		assert_eq!(serde_json::to_value(ShareRole::First).unwrap(), "first");
		assert_eq!(serde_json::to_value(ShareRole::Second).unwrap(), "second");
	}
}
