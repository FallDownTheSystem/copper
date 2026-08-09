//! The seam between this feature and the network, and its one real
//! implementation.
//!
//! Nothing in this file is unit-tested, matching `previews/net.rs`: a test here
//! would have to reach a real host, and a suite that does is a suite that fails
//! on an aeroplane. The [`Relay`] trait exists so that everything with logic in
//! it — the drain loop, the hole rule, the poison rule, the re-sync rules — is
//! tested against an in-memory fake instead. That is the `Pages` pattern
//! task-020 established, applied to the same problem.
//!
//! **`ureq`, not `reqwest`.** This runs on a plain OS thread, and
//! `reqwest::blocking` is documented to panic when built from inside a running
//! async runtime — which a Tauri app always has. `ureq` has no runtime to
//! conflict with, so that failure is unreachable however this is reached.
//!
//! **No redirects.** `resolve` has already checked that the relay URL is plain
//! `https` with no credentials; following a redirect would let the far end
//! decide where the bearer token goes next.

use std::time::Duration;

use copper_core::store::error::{Result, StoreError};
use serde::Deserialize;
use ureq::Agent;

use super::protocol::SHARE_MAX_PAYLOAD_BYTES;
use super::{HEAD_TIMEOUT, TRANSFER_TIMEOUT};

/// What the Worker said about a message it stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendAck {
	/// 204: stored, and the head pointer moved. The reader will see it.
	Delivered,
	/// 202: stored, but the head write failed, so nothing announces it yet. Not
	/// lost — the head is a high-water mark and the reader walks every sequence up
	/// to it, so the next successful send announces this message too.
	Unannounced,
}

/// Everything the poller and the send command ask of a relay.
///
/// Five methods, one per Worker route plus the acknowledged-cursor read that
/// `GET /head` serves for the reader's own mailbox.
pub trait Relay: Send + Sync {
	/// The highest sequence written to `mailbox`, or `None` when it is empty.
	fn head(&self, mailbox: &str) -> Result<Option<u64>>;
	/// The sealed bytes at `seq`, or `None` for a 404.
	fn fetch(&self, mailbox: &str, seq: u64) -> Result<Option<Vec<u8>>>;
	/// The highest sequence the reader of `mailbox` has acknowledged.
	fn acked(&self, mailbox: &str) -> Result<Option<u64>>;
	fn send(&self, mailbox: &str, seq: u64, body: &[u8]) -> Result<SendAck>;
	/// Deletes a consumed message and advances the acknowledged cursor.
	fn ack(&self, mailbox: &str, seq: u64) -> Result<()>;
}

/// A refused bearer token, kept distinct from every other failure.
///
/// The Settings view's **Test connection** has to be able to say "the relay
/// token is wrong" rather than "something went wrong", because that is the one
/// failure the user can fix from where they are standing.
pub const UNAUTHORISED: &str = "the relay refused this token";

/// Whether an error is the relay refusing the token.
///
/// A sentinel string rather than a second error type: everything in this feature
/// already returns `StoreError`, and adding a parallel enum would mean every
/// call site converting between the two.
pub fn is_unauthorised(err: &StoreError) -> bool {
	matches!(err, StoreError::Unavailable(message) if message == UNAUTHORISED)
}

/// Whether the request provably stored nothing.
///
/// Three classes, and telling them apart from a timeout is what makes the
/// difference between "send it again" and "it may already have arrived":
///
/// - **401 and 413.** The Worker replied, and replied before touching KV.
/// - **`Invalid`.** The relay refused the request outright.
/// - **`NotFound`.** The request never reached a server at all — no host, no
///   connection, no TLS. See [`transport`].
///
/// A send reported as `unknown` tells the user their note may have arrived,
/// which would stop them resending one that provably did not.
pub fn is_refusal(err: &StoreError) -> bool {
	is_unauthorised(err) || matches!(err, StoreError::Invalid(_) | StoreError::NotFound(_))
}

/// Generous for `{"head":"18446744073709551615"}` and nowhere near a size worth
/// buffering.
const COUNTER_MAX_BYTES: u64 = 4096;

/// The `{"head": "<digits>"}` reply, and the same shape for the ack cursor.
///
/// A **string**, never a JSON number: a 20-digit sequence is past JavaScript's
/// safe integer range, so the Worker cannot emit one as a number without losing
/// digits.
#[derive(Deserialize)]
struct Counter {
	head: Option<String>,
}

pub struct HttpRelay {
	agent: Agent,
	base: String,
	token: String,
}

impl HttpRelay {
	/// `base` is the validated relay URL, without its trailing slash.
	pub fn new(base: &str, token: &str) -> Self {
		Self {
			// One agent per relay rather than a process-wide one, because the token and
			// the base URL are part of what it is for and both can change under the
			// user. Building one costs no connection; the pool fills on first use.
			agent: Self::agent(TRANSFER_TIMEOUT),
			base: base.to_string(),
			token: token.to_string(),
		}
	}

	fn agent(timeout: Duration) -> Agent {
		Agent::config_builder()
			.timeout_global(Some(timeout))
			// See the module doc: the relay does not get to redirect the bearer token
			// somewhere else.
			.max_redirects(0)
			// Statuses are read rather than thrown, because 202, 404 and 401 are all
			// ordinary answers in this protocol rather than failures.
			.http_status_as_error(false)
			.build()
			.into()
	}

	fn url(&self, path: &str, mailbox: &str, seq: Option<u64>) -> String {
		match seq {
			Some(seq) => format!("{}{path}?box={mailbox}&seq={seq}", self.base),
			None => format!("{}{path}?box={mailbox}", self.base),
		}
	}

	/// Reads a counter from `GET /head`.
	///
	/// `which` selects the cursor: the default head pointer, or `ack` for the
	/// acknowledged one. The same route rather than two, so that the idle poll —
	/// the only call made every minute — costs exactly one KV read, and the rare
	/// re-sync costs one more.
	fn counter(&self, mailbox: &str, which: Option<&str>) -> Result<Option<u64>> {
		let mut url = self.url("/head", mailbox, None);
		if let Some(which) = which {
			url.push_str("&cursor=");
			url.push_str(which);
		}

		let mut response = Self::agent(HEAD_TIMEOUT)
			.get(url)
			.header("Authorization", self.bearer())
			.call()
			.map_err(transport)?;

		check_status(response.status().as_u16(), &[200])?;

		// An explicit limit on every response read, here as well as on the message
		// path: `ureq`'s default is 10 MB, and a counter reply is a few dozen bytes.
		// A relay answering this route with megabytes is either broken or hostile,
		// and either way there is nothing to gain by reading it.
		let counter: Counter = response
			.body_mut()
			.with_config()
			.limit(COUNTER_MAX_BYTES)
			.read_json()
			.map_err(|err| StoreError::Parse(format!("the relay's reply was not readable: {err}")))?;

		counter.head.map(|text| parse_counter(&text)).transpose()
	}

	fn bearer(&self) -> String {
		format!("Bearer {}", self.token)
	}
}

impl Relay for HttpRelay {
	fn head(&self, mailbox: &str) -> Result<Option<u64>> {
		self.counter(mailbox, None)
	}

	fn acked(&self, mailbox: &str) -> Result<Option<u64>> {
		self.counter(mailbox, Some("ack"))
	}

	fn fetch(&self, mailbox: &str, seq: u64) -> Result<Option<Vec<u8>>> {
		let mut response = self
			.agent
			.get(self.url("/msg", mailbox, Some(seq)))
			.header("Authorization", self.bearer())
			.call()
			.map_err(transport)?;

		let status = response.status().as_u16();
		if status == 404 {
			return Ok(None);
		}
		check_status(status, &[200])?;

		// Refused before a byte is read when the relay declares an oversized body,
		// and bounded again by the read itself — `Content-Length` is advisory and
		// free to lie, the same reasoning `attachments::read_capped` records.
		if let Some(declared) = response
			.headers()
			.get("content-length")
			.and_then(|value| value.to_str().ok())
			.and_then(|text| text.parse::<usize>().ok())
		{
			if declared > SHARE_MAX_PAYLOAD_BYTES {
				return Err(StoreError::Invalid(format!(
					"a message in this mailbox is {declared} bytes, over the \
					 {SHARE_MAX_PAYLOAD_BYTES} byte limit"
				)));
			}
		}

		// `ureq`'s reader caps at 10 MB by default, which is half the size a message
		// is allowed to be — so the limit is set explicitly on every read rather
		// than inherited. One byte over the cap, so an exactly-sized message is not
		// mistaken for an oversized one.
		let bytes = response
			.body_mut()
			.with_config()
			.limit((SHARE_MAX_PAYLOAD_BYTES + 1) as u64)
			.read_to_vec()
			.map_err(|err| StoreError::Io(format!("a message could not be read: {err}")))?;

		if bytes.len() > SHARE_MAX_PAYLOAD_BYTES {
			return Err(StoreError::Invalid(
				"a message in this mailbox is over the size limit".into(),
			));
		}
		Ok(Some(bytes))
	}

	fn send(&self, mailbox: &str, seq: u64, body: &[u8]) -> Result<SendAck> {
		let response = self
			.agent
			.post(self.url("/send", mailbox, Some(seq)))
			.header("Authorization", self.bearer())
			.header("Content-Type", "application/octet-stream")
			.send(body)
			.map_err(transport)?;

		match response.status().as_u16() {
			204 => Ok(SendAck::Delivered),
			202 => Ok(SendAck::Unannounced),
			413 => Err(StoreError::Invalid(
				"the relay refused this note as too large".into(),
			)),
			status => {
				check_status(status, &[204, 202])?;
				// Unreachable: `check_status` returns an error for anything not in the
				// accepted list, and both accepted values are matched above.
				Ok(SendAck::Delivered)
			}
		}
	}

	fn ack(&self, mailbox: &str, seq: u64) -> Result<()> {
		// The short timeout, not the transfer one: this carries a sequence number
		// and nothing else. `self.agent` is for the two requests that can be 20 MiB.
		let response = Self::agent(HEAD_TIMEOUT)
			.delete(self.url("/msg", mailbox, Some(seq)))
			.header("Authorization", self.bearer())
			.call()
			.map_err(transport)?;

		check_status(response.status().as_u16(), &[204])
	}
}

/// A decimal counter as a `u64`.
fn parse_counter(text: &str) -> Result<u64> {
	text.trim().parse::<u64>().map_err(|_| {
		StoreError::Parse("the relay reported a counter this build cannot read".into())
	})
}

/// Classifies a status the protocol did not expect.
fn check_status(status: u16, accepted: &[u16]) -> Result<()> {
	if accepted.contains(&status) {
		return Ok(());
	}
	if status == 401 || status == 403 {
		return Err(StoreError::Unavailable(UNAUTHORISED.into()));
	}
	Err(StoreError::Unavailable(format!(
		"the relay answered {status}"
	)))
}

/// A transport failure, worded without the URL or the token.
///
/// `ureq`'s own error text can carry the request URL, and the URL is the one
/// place a credential could ever appear — `resolve` refuses a URL with
/// credentials in it, so this is defence in depth rather than the only guard.
/// The message the user sees names the class of failure, not the request.
/// The variant decides more than the wording: [`is_refusal`] reads `NotFound` as
/// "this request never left", which is what lets a send say "try again" instead
/// of "it may have arrived".
fn transport(err: ureq::Error) -> StoreError {
	match err {
		ureq::Error::Timeout(_) => {
			StoreError::Unavailable("the relay did not answer in time".into())
		}
		// Nothing was sent: there was no host, no connection, or no TLS session to
		// send it over. A definite nothing, not an ambiguous one.
		ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => {
			StoreError::NotFound("the relay could not be reached".into())
		}
		ureq::Error::Tls(_) => {
			StoreError::NotFound("the relay's certificate could not be verified".into())
		}
		ureq::Error::TooManyRedirects => StoreError::NotFound(
			"the relay tried to redirect this request, which is not followed".into(),
		),
		other => StoreError::Unavailable(format!("the relay request failed ({})", kind_of(&other))),
	}
}

/// The variant name alone, never the payload — a payload can hold the URL.
fn kind_of(err: &ureq::Error) -> &'static str {
	match err {
		ureq::Error::StatusCode(_) => "unexpected status",
		ureq::Error::Http(_) => "malformed HTTP",
		ureq::Error::Io(_) => "connection lost",
		ureq::Error::Tls(_) => "TLS failed",
		ureq::Error::Protocol(_) => "protocol error",
		ureq::Error::BodyExceedsLimit(_) => "the reply was too large",
		_ => "unknown",
	}
}
