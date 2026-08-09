//! The command surface, as clap derive types. Pure data — nothing here opens a
//! file or reaches a store.
//!
//! Argument *groups* carry rules the command bodies would otherwise have to
//! re-check and re-word: `--done` and `--open` cannot both be given, a note body
//! comes either from arguments or from stdin and never both or neither, and
//! `copy` takes exactly one of its four selectors. Refusals from clap exit 2,
//! which is also `invalid`'s code — both mean "the request was malformed", so
//! sharing one code is honest rather than a collision.

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

/// The `copper` command.
///
/// `version` reads `CARGO_PKG_VERSION`, which is `version.workspace = true` —
/// one number for the app, the installer and this binary.
#[derive(Parser, Debug)]
#[command(
	name = "copper",
	// Pinned rather than taken from `argv[0]`, which is what clap does by
	// default. The cargo target is `copper-cli`, so without this every usage line
	// and every `--help` would read `copper-cli.exe` — a name that exists only
	// inside this repository. Installed, the file *is* `copper.exe`; pinning the
	// string means the two agree before task-025 renames anything.
	bin_name = "copper",
	version,
	about = "Read and edit Copper spaces from a terminal.",
	long_about = "Read and edit Copper spaces from a terminal.\n\n\
	              Writes go through the same compare-and-swap pipeline the app \
	              uses, so the two are safe to run at once and a running app picks \
	              a CLI edit up within about a second. One documented side effect: \
	              like any change made outside the app, a CLI write clears the \
	              app's in-memory undo history for that space.\n\n\
	              Which space a command works on is resolved per invocation: \
	              --space, then $COPPER_SPACE, then `copper space use`, then the \
	              app's own active space.",
	propagate_version = true
)]
pub struct Cli {
	#[command(subcommand)]
	pub command: Command,

	/// The space to work on, overriding every other source.
	#[arg(long, global = true, value_name = "PATH")]
	pub space: Option<PathBuf>,

	/// Emit a JSON object on stdout instead of human-readable text.
	///
	/// Every subcommand's own --help gives the exact shape it emits. Errors
	/// become {"kind","message"} on stderr, with the same exit code as ever.
	/// Clap's own usage errors stay plain text — they happen before any flag has
	/// been understood.
	#[arg(long, global = true)]
	pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
	/// Recent spaces, and which one this CLI works on.
	#[command(subcommand)]
	Space(SpaceCommand),
	/// The sections of a space.
	#[command(subcommand)]
	Section(SectionCommand),
	/// The notes of a space.
	#[command(subcommand)]
	Note(NoteCommand),
	/// Render notes to stdout, and optionally to the clipboard.
	///
	/// --json: {"format","text","clipboard"}
	/// "text" is the raw rendering in whatever shape --format produced: a string
	/// for markdown/list/bodies, a JSON array for --format json. "clipboard" is
	/// whether it reached the clipboard, not whether it was asked for.
	#[command(verbatim_doc_comment)]
	Copy(CopyArgs),
	/// Find notes whose bodies match a query.
	///
	/// --json: {"query","exact","results":[{"id","section","body"}]}
	#[command(verbatim_doc_comment)]
	Search(SearchArgs),
	/// Work with the files attached to a note.
	#[command(subcommand)]
	Attachment(AttachmentCommand),
}

// --- space ---------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum SpaceCommand {
	/// The app's recent spaces, with whether each can be opened right now.
	///
	/// --json: {"spaces":[{"path","active","name","availability"}]}
	/// "active" is the app's own active space, which is a different question
	/// from the one `space current` answers. "name" is the document's own name,
	/// or null when the file could not be read. "availability" is
	/// {"state":"available"} or {"state":"unavailable","reason","message"}.
	#[command(verbatim_doc_comment)]
	List,
	/// Point this CLI at a space. Does not change what the running app has open.
	///
	/// --json: {"space":"<path>"}
	#[command(verbatim_doc_comment)]
	Use {
		#[arg(value_name = "PATH")]
		path: PathBuf,
	},
	/// Print the space this CLI is pointed at.
	///
	/// --json: {"space":"<path>"|null}
	#[command(verbatim_doc_comment)]
	Current,
	/// Forget this CLI's own selection and fall back through the chain.
	///
	/// --json: {"space":null}
	#[command(verbatim_doc_comment)]
	Clear,
	/// Create a new, empty space. Refuses to overwrite an existing file.
	///
	/// --json: {"path","id","name"}
	#[command(verbatim_doc_comment)]
	Create {
		#[arg(value_name = "PATH")]
		path: PathBuf,
		/// The document's name. Defaults to the file name without its extension.
		#[arg(long, value_name = "NAME")]
		name: Option<String>,
	},
}

// --- section -------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum SectionCommand {
	/// The sections of the space, in document order.
	///
	/// --json: {"sections":[{"id","name","order","active","notes"}]}
	/// "notes" is how many notes are in the section; "active" marks the one an
	/// unqualified `note add` lands in.
	#[command(verbatim_doc_comment)]
	List,
	/// Add a section, and make it the active one.
	///
	/// Activation is the store's behaviour, not a choice made here, and it is
	/// worth knowing: an unqualified `note add` lands in the active section, so
	/// adding a section changes where the next one goes. Pass `--section` to say
	/// otherwise.
	///
	/// --json: {"id":"sec_…"}
	#[command(verbatim_doc_comment)]
	Add {
		#[arg(value_name = "NAME")]
		name: String,
	},
	/// Rename a section.
	///
	/// --json: {"id":"sec_…"}
	#[command(verbatim_doc_comment)]
	Rename {
		/// A section id (`sec_…`) or an unambiguous, case-insensitive name.
		#[arg(value_name = "REF")]
		reference: String,
		#[arg(value_name = "NAME")]
		name: String,
	},
	/// Delete a section and every note in it.
	///
	/// --json: {"id":"sec_…"}
	#[command(verbatim_doc_comment)]
	Delete {
		#[arg(value_name = "REF")]
		reference: String,
	},
}

// --- note ----------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum NoteCommand {
	/// The notes of the space, in document order.
	///
	/// --json: {"notes":[{"id","section","sectionName","order","done","body",
	/// "attachments","created","updated"}]}
	/// "attachments" is always present, empty array included, and each entry is
	/// {"id","file","name","mime","bytes","width","height"}.
	#[command(verbatim_doc_comment)]
	List(NoteListArgs),
	/// Add a note.
	///
	/// --json: {"id":"nte_…"}
	#[command(verbatim_doc_comment)]
	Add(NoteAddArgs),
	/// Replace a note's body.
	///
	/// --json: {"id":"nte_…"}
	#[command(verbatim_doc_comment)]
	Edit(NoteEditArgs),
	/// Delete notes.
	///
	/// --json: {"ids":["nte_…"]}
	#[command(verbatim_doc_comment)]
	Delete {
		#[arg(value_name = "ID", required = true, num_args = 1..)]
		ids: Vec<String>,
	},
	/// Move notes into a section.
	///
	/// --json: {"ids":["nte_…"]}
	#[command(name = "move", verbatim_doc_comment)]
	Move {
		#[arg(value_name = "ID", required = true, num_args = 1..)]
		ids: Vec<String>,
		#[arg(long, value_name = "REF")]
		section: String,
	},
	/// Mark notes done.
	///
	/// --json: {"ids":["nte_…"]}
	#[command(verbatim_doc_comment)]
	Done {
		#[arg(value_name = "ID", required = true, num_args = 1..)]
		ids: Vec<String>,
	},
	/// Mark notes not done.
	///
	/// --json: {"ids":["nte_…"]}
	#[command(verbatim_doc_comment)]
	Undone {
		#[arg(value_name = "ID", required = true, num_args = 1..)]
		ids: Vec<String>,
	},
	/// Merge notes into one, joining their bodies with a blank line.
	///
	/// --json: {"ids":["nte_…"]}
	#[command(verbatim_doc_comment)]
	Merge {
		#[arg(value_name = "ID", required = true, num_args = 1..)]
		ids: Vec<String>,
	},
}

#[derive(Args, Debug)]
pub struct NoteListArgs {
	/// Only notes in this section.
	#[arg(long, value_name = "REF")]
	pub section: Option<String>,
	#[command(flatten)]
	pub state: DoneFilter,
	/// Stop after this many notes.
	#[arg(long, value_name = "N")]
	pub limit: Option<usize>,
}

#[derive(Args, Debug)]
#[command(group = ArgGroup::new("note-body").required(true).multiple(false))]
pub struct NoteAddArgs {
	/// The body. Several arguments are joined with single spaces.
	#[arg(value_name = "BODY", group = "note-body", num_args = 1..)]
	pub body: Vec<String>,
	/// Read the body from standard input instead.
	#[arg(long, group = "note-body")]
	pub stdin: bool,
	/// The section to add to. Defaults to the space's active section.
	#[arg(long, value_name = "REF")]
	pub section: Option<String>,
	/// Put the note at the top of its section rather than the bottom.
	#[arg(long)]
	pub top: bool,
}

#[derive(Args, Debug)]
#[command(group = ArgGroup::new("edit-body").required(true).multiple(false))]
pub struct NoteEditArgs {
	/// A note id, or an unambiguous prefix of the part after `nte_`.
	#[arg(value_name = "ID")]
	pub id: String,
	/// The replacement body. Several arguments are joined with single spaces.
	#[arg(value_name = "BODY", group = "edit-body", num_args = 1..)]
	pub body: Vec<String>,
	/// Read the replacement body from standard input instead.
	#[arg(long, group = "edit-body")]
	pub stdin: bool,
}

/// `--done` and `--open`, which cannot both be true.
///
/// Two flags rather than one `--state <done|open>` because that is what the spec
/// names, and because `--open` reads better than `--state open` in a shell.
#[derive(Args, Debug)]
#[command(group = ArgGroup::new("done-filter").multiple(false))]
pub struct DoneFilter {
	/// Only notes that are done.
	#[arg(long, group = "done-filter")]
	pub done: bool,
	/// Only notes that are not done.
	#[arg(long, group = "done-filter")]
	pub open: bool,
}

impl DoneFilter {
	/// `None` when neither flag was given, which means "both kinds".
	pub fn wanted(&self) -> Option<bool> {
		match (self.done, self.open) {
			(true, _) => Some(true),
			(_, true) => Some(false),
			_ => None,
		}
	}
}

// --- copy ----------------------------------------------------------------------

#[derive(Args, Debug)]
#[command(group = ArgGroup::new("selection").required(true).multiple(false))]
pub struct CopyArgs {
	/// Note ids, or unambiguous prefixes of the part after `nte_`.
	#[arg(value_name = "ID", group = "selection", num_args = 1..)]
	pub ids: Vec<String>,
	/// Every note in this section.
	#[arg(long, value_name = "REF", group = "selection")]
	pub section: Option<String>,
	/// Every note in the space.
	#[arg(long, group = "selection")]
	pub all: bool,
	/// Every note matching this query.
	#[arg(long, value_name = "QUERY", group = "selection")]
	pub query: Option<String>,

	/// What the rendered text looks like. This is a *content* choice, unrelated to
	/// the global --json flag, which wraps whatever it produces.
	#[arg(long, value_enum, default_value_t = CopyFormat::Markdown)]
	pub format: CopyFormat,
	/// Also place the rendering on the Windows clipboard. stdout is written
	/// either way.
	#[arg(long)]
	pub clipboard: bool,
	/// With --query: plain case-insensitive substring matching instead of fuzzy.
	#[arg(long)]
	pub exact: bool,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyFormat {
	/// `# Section` headings and `- [ ] ` task items. What the app's "Copy as
	/// Markdown" produces.
	Markdown,
	/// A flat `- ` list, no headings and no checkboxes.
	List,
	/// The raw bodies, joined by a blank line. What the app's "Copy" produces.
	Bodies,
	/// A JSON array of `{ id, done, body }`.
	Json,
}

impl CopyFormat {
	pub fn name(self) -> &'static str {
		match self {
			Self::Markdown => "markdown",
			Self::List => "list",
			Self::Bodies => "bodies",
			Self::Json => "json",
		}
	}
}

// --- search --------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct SearchArgs {
	/// The query. Whitespace is stripped, so `http req` is one character
	/// sequence and matches notes the words are merely spread across.
	#[arg(value_name = "QUERY")]
	pub query: String,
	/// Only notes in this section.
	#[arg(long, value_name = "REF")]
	pub section: Option<String>,
	#[command(flatten)]
	pub state: DoneFilter,
	/// Plain case-insensitive substring matching instead of fuzzy.
	#[arg(long)]
	pub exact: bool,
	/// Stop after this many notes.
	#[arg(long, value_name = "N")]
	pub limit: Option<usize>,
}

// --- attachment ----------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum AttachmentCommand {
	/// Copy a note's attachments out, under their original names.
	///
	/// --json: {"exported":[{"name","path","bytes"}],
	/// "failed":[{"name","message"}]}
	/// Exits 7 if any attachment failed, having exported and reported the rest.
	#[command(verbatim_doc_comment)]
	Export {
		/// A note id, or an unambiguous prefix of the part after `nte_`.
		#[arg(value_name = "NOTE-ID")]
		id: String,
		/// Where to write them. Defaults to the current directory.
		#[arg(long, value_name = "DIR")]
		out: Option<PathBuf>,
	},
}
