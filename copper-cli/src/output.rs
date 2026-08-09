//! What a command produces, rendered two ways.
//!
//! Every command body returns one [`Report`] and never writes to stdout itself.
//! That is the whole reason this module exists: a command that printed its own
//! human text *and* its own JSON would have two descriptions of one result, and
//! the two would drift the first time a field was added. Here they are two
//! functions over one value.
//!
//! The error side is the same idea. `StoreError::kind()` is the store's stable
//! discriminant, and [`exit_code`] is the only place it becomes a number.

use std::path::Path;

use copper_core::spaces::availability::Availability;
use copper_core::store::error::StoreError;
use serde::Serialize;
use serde_json::{json, Value};

/// `StoreError::kind()` → process exit code (spec 7).
///
/// `invalid` shares 2 with clap's usage errors, deliberately: both mean the
/// request was malformed, and a script that only wants to know "did I ask for
/// something impossible" should not have to tell a bad flag from a bad id.
pub fn exit_code(error: &StoreError) -> i32 {
	match error.kind() {
		"invalid" => 2,
		"not-found" => 3,
		"unavailable" => 4,
		"conflict" => 5,
		"parse" => 6,
		"io" => 7,
		// Unreachable while `StoreError` has six variants, and mapped rather than
		// panicked: a seventh added later should degrade to "malformed request",
		// not abort the process.
		_ => 2,
	}
}

/// The error envelope, on stderr. `kind: message` as text, `{kind, message}` as
/// JSON.
pub fn report_error(error: &StoreError, as_json: bool) {
	if as_json {
		let envelope = json!({ "kind": error.kind(), "message": error.message() });
		eprintln!("{envelope}");
	} else {
		eprintln!("{}: {}", error.kind(), error.message());
	}
}

// --- row types ------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceRow {
	pub path: String,
	/// Whether this is `settings.activeSpace` — the *app's* durable notion, which
	/// is a different question from the one `copper space current` answers.
	pub active: bool,
	pub name: Option<String>,
	pub availability: Availability,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionRow {
	pub id: String,
	pub name: String,
	pub order: i64,
	/// Whether this is the document's `activeSection`, which is where an
	/// unqualified `note add` lands.
	pub active: bool,
	pub notes: usize,
}

/// A note as the CLI reports it.
///
/// `attachments` is always present, even when empty. The on-disk format omits the
/// key entirely in that case (`skip_serializing_if`), but this is CLI output
/// rather than the store's serialisation, and a consumer should not have to
/// special-case a missing array.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRow {
	pub id: String,
	pub section: String,
	pub section_name: String,
	pub order: i64,
	pub done: bool,
	pub body: String,
	pub attachments: Vec<AttachmentRow>,
	pub created: String,
	pub updated: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRow {
	pub id: String,
	pub file: String,
	pub name: String,
	pub mime: String,
	pub bytes: u64,
	pub width: Option<u32>,
	pub height: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRow {
	pub id: String,
	pub section: String,
	pub body: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRow {
	pub name: String,
	pub path: String,
	pub bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedExport {
	pub name: String,
	pub message: String,
}

// --- the reports -----------------------------------------------------------------

pub enum Report {
	Spaces(Vec<SpaceRow>),
	/// `space use` / `current` / `clear`. `None` means no CLI selection.
	Selection(Option<String>),
	Created {
		path: String,
		id: String,
		name: String,
	},
	Sections(Vec<SectionRow>),
	Notes(Vec<NoteRow>),
	/// A mutation that produced one new id.
	Id(String),
	/// A mutation over a list of notes, echoing what it touched.
	Ids(Vec<String>),
	Search {
		query: String,
		exact: bool,
		results: Vec<SearchRow>,
	},
	Copy {
		format: &'static str,
		/// The rendered payload in whatever shape `--format` produced: a JSON
		/// string for the text formats, a JSON array for `--format json`. Not
		/// always a string, so that `--format json --json` nests rather than
		/// double-encoding.
		text: Value,
		/// Whether `--clipboard` was given. Set by the command.
		clipboard_wanted: bool,
		/// Whether the rendering actually reached the clipboard. Set by `main`,
		/// which decides *when* the write happens relative to stdout.
		clipboard: bool,
		/// Why it did not, when `--clipboard` was asked for and failed. Reported
		/// after the rendering is printed, never instead of it.
		clipboard_error: Option<String>,
	},
	Export {
		exported: Vec<ExportRow>,
		failed: Vec<FailedExport>,
	},
}

impl Report {
	/// The failure to report once this report has been printed, if any.
	///
	/// Two commands can partly succeed — an export whose third attachment is
	/// missing, a copy whose clipboard write was refused — and both must still
	/// deliver what they did produce. So the failure travels beside the report
	/// rather than in place of it, and `main` prints the report first.
	///
	/// It comes back as a `StoreError` rather than as loose strings so that it
	/// goes out through [`report_error`], the same door every other failure uses.
	/// That is what keeps `--json` honest: the envelope on stderr is the
	/// documented `{kind, message}` either way, rather than prose under one flag
	/// and nothing at all under the other. It matters most for `copy`, where
	/// `"clipboard": false` on its own cannot say *why*, and is otherwise
	/// indistinguishable from a copy that never asked for the clipboard.
	pub fn deferred(&self) -> Option<StoreError> {
		match self {
			Self::Export { failed, .. } if !failed.is_empty() => {
				Some(StoreError::Io(
					failed
						.iter()
						.map(|row| format!("could not export {}: {}", row.name, row.message))
						.collect::<Vec<_>>()
						.join("\n"),
				))
			}
			Self::Copy {
				clipboard_error: Some(why),
				..
			} => Some(StoreError::Io(why.clone())),
			_ => None,
		}
	}

	/// The exact bytes `copy` was asked to produce, with nothing added.
	///
	/// Separate from [`Report::to_text`] because this is the one report whose
	/// stdout is a *payload* rather than a listing: `copper copy --format bodies >
	/// notes.md` must write the notes and not one byte more, and the same string
	/// is what reaches the clipboard.
	pub fn payload(&self) -> Option<String> {
		match self {
			Self::Copy { text, .. } => Some(match text {
				Value::String(rendered) => rendered.clone(),
				other => other.to_string(),
			}),
			_ => None,
		}
	}

	/// Whether `--clipboard` was given, which is not the same question as whether
	/// the write succeeded.
	pub fn wants_clipboard(&self) -> bool {
		matches!(self, Self::Copy { clipboard_wanted: true, .. })
	}

	/// Records what the clipboard write did. Called by `main`, which owns the
	/// ordering.
	pub fn set_clipboard(&mut self, outcome: Result<(), StoreError>) {
		if let Self::Copy {
			clipboard,
			clipboard_error,
			..
		} = self
		{
			match outcome {
				Ok(()) => *clipboard = true,
				Err(err) => *clipboard_error = Some(err.message()),
			}
		}
	}
}

impl Report {
	pub fn to_json(&self) -> Value {
		match self {
			Self::Spaces(rows) => json!({ "spaces": rows }),
			Self::Selection(space) => json!({ "space": space }),
			Self::Created { path, id, name } => json!({ "path": path, "id": id, "name": name }),
			Self::Sections(rows) => json!({ "sections": rows }),
			Self::Notes(rows) => json!({ "notes": rows }),
			Self::Id(id) => json!({ "id": id }),
			Self::Ids(ids) => json!({ "ids": ids }),
			Self::Search {
				query,
				exact,
				results,
			} => json!({ "query": query, "exact": exact, "results": results }),
			// `clipboard_error` is deliberately not a key: the documented shape is
			// three fields, and a failed write is already reported by
			// `clipboard: false` plus a non-zero exit.
			Self::Copy {
				format,
				text,
				clipboard,
				..
			} => json!({ "format": format, "text": text, "clipboard": clipboard }),
			Self::Export { exported, failed } => json!({ "exported": exported, "failed": failed }),
		}
	}

	/// The human rendering. `None` means "print nothing", which is not the same as
	/// an empty string — `copy` writes its payload verbatim and an empty selection
	/// legitimately produces no bytes at all.
	pub fn to_text(&self) -> String {
		match self {
			Self::Spaces(rows) => {
				if rows.is_empty() {
					return "No recent spaces.".into();
				}
				rows.iter()
					.map(|row| {
						let marker = if row.active { "*" } else { " " };
						// The document's own name where the probe could read one, and the
						// file stem where it could not, so an unavailable row still shows
						// something recognisable rather than a bare path. The JSON keeps
						// `null` there — a stem is a guess, and a consumer that wants one
						// can take it from `path` itself.
						let stem = || {
							Path::new(&row.path)
								.file_stem()
								.map(|stem| stem.to_string_lossy().into_owned())
								.unwrap_or_else(|| "?".to_string())
						};
						let name = row.name.clone().unwrap_or_else(stem);
						match row.availability.message() {
							Some(why) => format!("{marker} {name}\t{}\t{why}", row.path),
							None => format!("{marker} {name}\t{}", row.path),
						}
					})
					.collect::<Vec<_>>()
					.join("\n")
			}
			Self::Selection(space) => match space {
				Some(path) => path.clone(),
				None => "No space is selected for the CLI.".into(),
			},
			Self::Created { path, name, .. } => format!("Created {name} at {path}"),
			Self::Sections(rows) => {
				if rows.is_empty() {
					return "No sections.".into();
				}
				rows.iter()
					.map(|row| {
						let marker = if row.active { "*" } else { " " };
						let plural = if row.notes == 1 { "note" } else { "notes" };
						format!(
							"{marker} {}\t{}\t{} {plural}",
							row.id, row.name, row.notes
						)
					})
					.collect::<Vec<_>>()
					.join("\n")
			}
			Self::Notes(rows) => {
				if rows.is_empty() {
					return "No notes.".into();
				}
				rows.iter().map(note_line).collect::<Vec<_>>().join("\n")
			}
			Self::Id(id) => id.clone(),
			Self::Ids(ids) => ids.join("\n"),
			Self::Search { results, .. } => {
				if results.is_empty() {
					return "No matches.".into();
				}
				results
					.iter()
					.map(|row| format!("{}\t{}", row.id, one_line(&row.body)))
					.collect::<Vec<_>>()
					.join("\n")
			}
			// The payload verbatim, because the point of `copy` is that its stdout
			// can be piped somewhere. Nothing is added around it. Through
			// [`Report::payload`] rather than a second unwrapping of `text`, so the
			// bytes this prints and the bytes that reach the clipboard cannot come
			// to differ.
			Self::Copy { .. } => self.payload().unwrap_or_default(),
			Self::Export { exported, failed } => {
				let mut lines: Vec<String> = exported
					.iter()
					.map(|row| format!("{}\t{}", row.name, row.path))
					.collect();
				if lines.is_empty() && failed.is_empty() {
					lines.push("This note has no attachments.".into());
				}
				lines.join("\n")
			}
		}
	}
}

fn note_line(row: &NoteRow) -> String {
	let box_ = if row.done { "[x]" } else { "[ ]" };
	let attachments = match row.attachments.len() {
		0 => String::new(),
		count => format!("\t({count} attached)"),
	};
	format!(
		"{box_} {}\t{}\t{}{attachments}",
		row.id,
		row.section_name,
		one_line(&row.body)
	)
}

/// A body on one line, so the listing stays one note per row.
///
/// Truncation is by characters rather than bytes — a body can hold anything —
/// and the marker is a plain ASCII ellipsis so the output survives a console
/// codepage that cannot render `…`.
fn one_line(body: &str) -> String {
	let flattened: String = body
		.chars()
		.map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
		.collect();
	let trimmed = flattened.trim();
	let mut out: String = trimmed.chars().take(80).collect();
	// `nth(80)` rather than a full count: the question is whether an 81st character
	// exists, and a long body should not be walked to its end to answer it.
	if trimmed.chars().nth(80).is_some() {
		out.push_str("...");
	}
	out
}
