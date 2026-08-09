//! Finding the space in a command line, and nothing else.
//!
//! Pure: no filesystem is consulted, so a path that does not exist parses
//! exactly like one that does and the failure is reported by whoever tries to
//! open it. That is deliberate — the two entry points differ in their `cwd`, and
//! testing this needs neither of them to be real.

use std::path::{Path, PathBuf};

use super::paths::is_rooted;

const EXTENSION: &str = "copper";

/// The first `.copper` argument, resolved against `cwd` when relative.
///
/// **Every argument is scanned, from index 0** — not `skip(1)` plus an
/// is-this-an-executable heuristic. Both entry points are confirmed to include
/// the executable at `argv[0]`: `std::env::args()` does, and the single-instance
/// plugin forwards its own `std::env::args()` unchanged. But an executable path
/// is not a `.copper` path, so scanning from zero is correct for both shapes and
/// for a hypothetical third that omits it, while positional skipping is brittle
/// against all three.
pub fn space_path_from_args(args: &[String], cwd: &Path) -> Option<PathBuf> {
	args.iter()
		.filter(|arg| !is_flag(arg))
		.map(PathBuf::from)
		.find(|path| has_copper_extension(path))
		.map(|path| resolve(path, cwd))
}

/// A defensive convention rather than a response to anything observed: nothing
/// documents Tauri or WebView2 injecting argv flags, and no evidence says they
/// do. It costs one line.
fn is_flag(arg: &str) -> bool {
	arg.starts_with('-')
}

fn has_copper_extension(path: &Path) -> bool {
	path.extension()
		.is_some_and(|ext| ext.eq_ignore_ascii_case(EXTENSION))
}

/// A path that carries its own prefix or root is left alone.
///
/// `PathBuf::join` replaces the whole path when the argument has a prefix, so
/// `cwd.join("C:notes.copper")` would silently discard `cwd` and produce
/// something neither the user nor the caller meant. Drive-relative and
/// root-relative shapes are left for `comparison_key` and `canonicalize` to
/// resolve against the bases Windows actually uses.
fn resolve(path: PathBuf, cwd: &Path) -> PathBuf {
	if is_rooted(&path) {
		path
	} else {
		cwd.join(path)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const EXE: &str = r"C:\Program Files\Copper\copper.exe";

	fn parse(args: &[&str], cwd: &str) -> Option<PathBuf> {
		let owned: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
		space_path_from_args(&owned, Path::new(cwd))
	}

	#[test]
	fn an_absolute_path_after_the_executable_is_taken_as_is() {
		assert_eq!(
			parse(&[EXE, r"D:\notes\work.copper"], r"C:\somewhere"),
			Some(PathBuf::from(r"D:\notes\work.copper"))
		);
	}

	/// The NSIS template already quotes `%1`, so a document path with spaces
	/// arrives as one argument. Nothing on our side has to reassemble it.
	#[test]
	fn a_path_containing_spaces_arrives_as_one_argument() {
		assert_eq!(
			parse(&[EXE, r"D:\my notes\a b.copper"], r"C:\x"),
			Some(PathBuf::from(r"D:\my notes\a b.copper"))
		);
	}

	#[test]
	fn a_relative_path_resolves_against_the_supplied_cwd() {
		assert_eq!(
			parse(&[EXE, r"sub\work.copper"], r"D:\projects"),
			Some(PathBuf::from(r"D:\projects\sub\work.copper"))
		);
	}

	#[test]
	fn a_non_ascii_path_survives() {
		assert_eq!(
			parse(&[EXE, r"D:\notes\päivä-日本.copper"], r"C:\x"),
			Some(PathBuf::from(r"D:\notes\päivä-日本.copper"))
		);
	}

	#[test]
	fn a_unc_path_is_not_joined_onto_the_working_directory() {
		assert_eq!(
			parse(&[EXE, r"\\server\share\notes.copper"], r"C:\x"),
			Some(PathBuf::from(r"\\server\share\notes.copper"))
		);
	}

	#[test]
	fn the_extension_match_is_case_insensitive() {
		assert_eq!(
			parse(&[EXE, r"D:\NOTES\WORK.COPPER"], r"C:\x"),
			Some(PathBuf::from(r"D:\NOTES\WORK.COPPER"))
		);
	}

	#[test]
	fn anything_that_is_not_a_copper_file_is_ignored() {
		assert_eq!(parse(&[EXE, r"D:\notes\work.txt"], r"C:\x"), None);
		assert_eq!(parse(&[EXE], r"C:\x"), None);
		assert_eq!(parse(&[], r"C:\x"), None);
	}

	#[test]
	fn flag_shaped_arguments_are_skipped() {
		assert_eq!(
			parse(&[EXE, "--verbose", "-x", "--", r"D:\a.copper"], r"C:\x"),
			Some(PathBuf::from(r"D:\a.copper"))
		);
		// A flag that happens to end in .copper is still a flag.
		assert_eq!(parse(&[EXE, "--config=a.copper"], r"C:\x"), None);
	}

	/// A16b. Nothing is stripped from either shape, so the parser must work with
	/// the executable present and absent alike.
	#[test]
	fn both_argv_shapes_reach_the_same_answer() {
		let with_exe = parse(&[EXE, r"D:\a.copper"], r"C:\x");
		let without = parse(&[r"D:\a.copper"], r"C:\x");
		assert_eq!(with_exe, without);
		assert_eq!(with_exe, Some(PathBuf::from(r"D:\a.copper")));
	}

	#[test]
	fn the_first_copper_argument_wins() {
		assert_eq!(
			parse(&[EXE, r"D:\first.copper", r"D:\second.copper"], r"C:\x"),
			Some(PathBuf::from(r"D:\first.copper"))
		);
	}
}
