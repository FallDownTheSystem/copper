//! `copper` — the headless half of Copper.
//!
//! **No `windows_subsystem` attribute.** The app's `main.rs` carries one, and a
//! CLI that copied it would have no console in a release build: `println!` would
//! go nowhere and every diagnostic would vanish. For the same reason nothing here
//! uses `diagnostics::log`, which routes to `OutputDebugStringW` — this process
//! talks to a terminal, through ordinary stdout and stderr.
//!
//! The shape of every invocation is the same: parse, resolve a space, open it
//! headless, run one command, render one [`Report`], exit with a code derived
//! from the error kind. Commands never print; `output.rs` does.

mod cli;
mod commands;
mod output;
mod resolve;

use std::io::{IsTerminal, Write};

use clap::Parser;
use copper_core::store::error::Result;

use cli::{Cli, Command};
use output::Report;

fn main() {
	// Clap exits on its own for `--help`, `--version` and usage errors, with 0 for
	// the first two and 2 for the last.
	let cli = Cli::parse();

	match run(&cli) {
		Ok(mut report) => {
			// **The clipboard's position relative to stdout depends on the output
			// contract, and there is no single order that satisfies both.**
			//
			// Plain output: stdout first. The rendering is what the user asked for,
			// and a clipboard held open by another process must not delay it or,
			// worse, replace it with an error.
			//
			// `--json`: the clipboard first, because the envelope reports
			// `"clipboard"` and cannot say truthfully whether the write landed until
			// it has been attempted. A caller that asked for a machine-readable
			// answer is better served by an accurate one than by a fast one.
			if report.wants_clipboard() && cli.json {
				place(&mut report);
			}
			emit(&report, cli.json);
			if report.wants_clipboard() && !cli.json {
				place(&mut report);
			}

			// A command that partly succeeded still reports what it did not do —
			// through the same `{kind, message}` envelope every other failure uses,
			// so `--json` gets JSON on stderr rather than prose or silence.
			if let Some(deferred) = report.deferred() {
				output::report_error(&deferred, cli.json);
				std::process::exit(output::exit_code(&deferred));
			}
		}
		Err(error) => {
			output::report_error(&error, cli.json);
			std::process::exit(output::exit_code(&error));
		}
	}
}

fn place(report: &mut Report) {
	let Some(payload) = report.payload() else {
		return;
	};
	report.set_clipboard(commands::copy::place_on_clipboard(&payload));
}

fn emit(report: &Report, as_json: bool) {
	if as_json {
		println!("{}", report.to_json());
		return;
	}

	// `copy`'s stdout is a **payload**, not a listing: `copper copy --format
	// bodies > notes.md` has to write the notes and not one byte more, and the
	// same string is what went to the clipboard. So the bytes go out exactly, and
	// the newline that makes a terminal readable is added only when stdout *is* a
	// terminal — where nothing is capturing it and the next shell prompt would
	// otherwise land mid-line.
	if let Some(payload) = report.payload() {
		let mut out = std::io::stdout().lock();
		let _ = out.write_all(payload.as_bytes());
		if out.is_terminal() {
			let _ = out.write_all(b"\n");
		}
		let _ = out.flush();
		return;
	}

	let text = report.to_text();
	if !text.is_empty() {
		println!("{text}");
	}
}

/// Dispatch.
///
/// The space is resolved and opened **only for the commands that need a
/// document**. `space list`, `space use`, `space current`, `space clear` and
/// `space create` are all about which spaces exist rather than about what is in
/// one, and resolving a space for them would make `copper space use` fail on a
/// machine that has no space selected yet — which is the one machine where it is
/// most needed.
fn run(cli: &Cli) -> Result<Report> {
	if let Command::Space(command) = &cli.command {
		return commands::space::run(command);
	}

	let path = resolve::space(cli.space.as_deref())?;
	let mut store = resolve::open(&path)?;

	match &cli.command {
		// Handled above; matched again because `Command` is not split into two
		// enums, and splitting it to please this match would put the CLI's grammar
		// in two places.
		Command::Space(command) => commands::space::run(command),
		Command::Section(command) => commands::section::run(&mut store, command),
		Command::Note(command) => commands::note::run(&mut store, command),
		Command::Copy(args) => commands::copy::run(&store, args),
		Command::Search(args) => commands::search::run(&store, args),
		Command::Attachment(cli::AttachmentCommand::Export { id, out }) => {
			commands::attachment::run(&store, id, out.as_deref())
		}
	}
}
