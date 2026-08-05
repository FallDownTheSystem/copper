//! Path identity for the switcher, and the one rule that makes it work:
//! **comparison never touches the filesystem.**
//!
//! The paths this layer compares are frequently the *unavailable* ones — a file
//! that has been deleted, a share that is not mounted. `fs::canonicalize` cannot
//! resolve the first and blocks on the second, so it is reserved for a path that
//! has just been opened successfully (the store does that, in `store::canonical`)
//! and is never used here.
//!
//! What that buys and what it costs are both worth stating. It buys a key that
//! answers instantly for a path on a drive that does not exist. It costs alias
//! resolution: two paths that reach one file through an 8.3 short name, a
//! junction, a `subst` drive, a symlink or a hard link do **not** compare equal,
//! so the already-active no-op can miss such an alias and perform a real switch
//! instead. The consequence there is a redundant reload, not lost data, and the
//! case is rare — whereas a missing file is the case this task exists to handle.

use std::path::{Component, Path, PathBuf, Prefix};

/// The variable the design's example `settings.json` shows a recents entry
/// carrying. Tolerated on input only — nothing here ever writes one.
const APPDATA: &str = "%APPDATA%";

/// The lexical identity of a path: what dedupe, promotion, removal and the
/// already-active check all compare.
///
/// Only a **leading** `%APPDATA%` is expanded, deliberately. `%` is a legal
/// character in a Windows filename, so a general `%VAR%` pass would mangle
/// `C:\notes\100%\done.copper` — or any folder someone named with percent signs
/// — into something that matches nothing.
pub fn comparison_key(path: &Path) -> String {
	let text = expand_appdata(&path.to_string_lossy());
	let text = crate::store::strip_verbatim_str(&text).replace('/', "\\");

	let (root, rest) = split_root(&text);
	let mut components: Vec<&str> = Vec::new();
	for component in rest.split('\\') {
		match component {
			"" | "." => {}
			// Never past the root: `C:\..\x` is `C:\x`, as Windows resolves it.
			".." => {
				components.pop();
			}
			other => components.push(other),
		}
	}

	let joined = if components.is_empty() {
		root
	} else if root.ends_with('\\') {
		format!("{root}{}", components.join("\\"))
	} else {
		// A UNC root is `\\server\share` with no trailing separator of its own.
		format!("{root}\\{}", components.join("\\"))
	};

	// ASCII folding, matching the store's own `settings::same_path`. Full Unicode
	// case folding would need a dependency to do correctly rather than
	// approximately, and the cases this has to catch — drive letters and typed
	// path segments — are ASCII.
	joined.to_ascii_uppercase()
}

/// Whether two paths name the same file, lexically. Used everywhere identity is
/// compared.
pub fn same_path(a: &Path, b: &Path) -> bool {
	comparison_key(a) == comparison_key(b)
}

/// The user-facing form: the path as supplied, with any verbatim prefix removed.
///
/// Never rebuilt from the comparison key — that is upper-cased and would shout.
/// The display form is a session-lifetime nicety for the entry the user just
/// picked; `settings.json` persists one string per entry, so after a restart the
/// display path *is* the stored canonical path.
pub fn display_path(path: &Path) -> String {
	crate::store::strip_verbatim_str(&path.to_string_lossy())
}

/// Splits a path into the part that must survive normalisation untouched and the
/// part that gets `.`/`..` resolution.
///
/// The four shapes that are not simply "absolute or relative", each of which
/// resolves against a different base:
///
/// - `\\server\share\x` — the root is the server *and* the share. Collapsing the
///   leading separators or trimming the trailing one off a bare `\\server\share`
///   produces a path that names nothing.
/// - `C:\x` — an ordinary rooted path.
/// - `C:x` — drive-*relative*, resolved against that drive's own current
///   directory, which Windows keeps in a hidden `=C:` environment variable.
/// - `\x` — rooted on the *current* drive.
fn split_root(text: &str) -> (String, &str) {
	if let Some(rest) = text.strip_prefix("\\\\") {
		// Server and share together are the root; either alone is not a location.
		let mut parts = rest.splitn(3, '\\');
		let server = parts.next().unwrap_or_default();
		let share = parts.next().unwrap_or_default();
		let tail = parts.next().unwrap_or_default();
		return (format!("\\\\{server}\\{share}"), tail);
	}

	let bytes = text.as_bytes();
	if bytes.len() >= 2 && bytes[1] == b':' && (bytes[0] as char).is_ascii_alphabetic() {
		let letter = bytes[0] as char;
		let rest = &text[2..];
		return match rest.strip_prefix('\\') {
			Some(tail) => (format!("{letter}:\\"), tail),
			// Drive-relative. The per-drive current directory falls back to the
			// drive root, which is what a fresh process sees for a drive it has not
			// visited.
			None => {
				let base = drive_current_dir(letter);
				(base, rest)
			}
		};
	}

	if let Some(tail) = text.strip_prefix('\\') {
		return (current_drive_root(), tail);
	}

	(working_directory(), text)
}

/// The working directory as a root, ending in a separator so joining is uniform.
fn working_directory() -> String {
	let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("\\"));
	let mut text = cwd.to_string_lossy().replace('/', "\\");
	if !text.ends_with('\\') {
		text.push('\\');
	}
	text
}

fn current_drive_root() -> String {
	let cwd = working_directory();
	match cwd.as_bytes().first() {
		Some(letter) if (*letter as char).is_ascii_alphabetic() && cwd.as_bytes().get(1) == Some(&b':') => {
			format!("{}:\\", *letter as char)
		}
		_ => "\\".to_string(),
	}
}

/// Windows records a current directory per drive in an environment variable
/// whose name begins with `=`. Absent — the drive has not been visited by this
/// process — the drive root is the answer.
fn drive_current_dir(letter: char) -> String {
	let key = format!("={}:", letter.to_ascii_uppercase());
	match std::env::var(&key) {
		Ok(value) if !value.is_empty() => {
			let mut text = value.replace('/', "\\");
			if !text.ends_with('\\') {
				text.push('\\');
			}
			text
		}
		_ => format!("{}:\\", letter.to_ascii_uppercase()),
	}
}

fn expand_appdata(text: &str) -> String {
	let Some(rest) = strip_prefix_ignore_ascii_case(text, APPDATA) else {
		return text.to_string();
	};
	match std::env::var("APPDATA") {
		Ok(value) if !value.is_empty() => format!("{}{rest}", value.trim_end_matches(['\\', '/'])),
		_ => text.to_string(),
	}
}

fn strip_prefix_ignore_ascii_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
	let head = text.get(..prefix.len())?;
	head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
}

/// Whether the path carries a Windows prefix or a root of its own, and so must
/// not be joined onto a working directory.
///
/// `Path::is_absolute` is false for both `C:x` and `\x`, and joining either onto
/// a `cwd` produces nonsense — `PathBuf::join` replaces the whole path when the
/// argument has a prefix, so `cwd.join("C:x")` silently discards `cwd`.
pub fn is_rooted(path: &Path) -> bool {
	matches!(
		path.components().next(),
		Some(Component::Prefix(_) | Component::RootDir)
	)
}

/// The drive letter a path names, when it names one. `None` for a UNC or
/// relative path, neither of which has a local volume to test.
pub fn drive_letter(path: &Path) -> Option<char> {
	match path.components().next() {
		Some(Component::Prefix(prefix)) => match prefix.kind() {
			Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
				Some((letter as char).to_ascii_uppercase())
			}
			_ => None,
		},
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn key(text: &str) -> String {
		comparison_key(Path::new(text))
	}

	fn same(a: &str, b: &str) -> bool {
		same_path(Path::new(a), Path::new(b))
	}

	#[test]
	fn case_and_separators_do_not_change_identity() {
		assert!(same(r"D:\X\a.copper", r"d:\x\A.COPPER"));
		assert!(same(r"D:\X\a.copper", "D:/X/a.copper"));
	}

	#[test]
	fn dot_and_dotdot_resolve_textually() {
		assert!(same(r"D:\X\.\sub\..\a.copper", r"D:\X\a.copper"));
		// Never past the root.
		assert!(same(r"D:\..\..\a.copper", r"D:\a.copper"));
	}

	#[test]
	fn a_trailing_separator_is_not_part_of_the_identity() {
		assert!(same(r"D:\X\sub\", r"D:\X\sub"));
	}

	/// The whole point of a lexical key: no filesystem is consulted, so a path on
	/// a drive letter that does not exist on this machine still compares.
	#[test]
	fn a_path_on_a_nonexistent_drive_still_compares() {
		assert!(same(r"Z:\nowhere\a.copper", r"z:\NOWHERE\A.COPPER"));
		assert!(!same(r"Z:\nowhere\a.copper", r"Y:\nowhere\a.copper"));
	}

	#[test]
	fn a_drive_root_keeps_its_separator() {
		assert_eq!(key(r"C:\"), r"C:\");
		assert_eq!(key(r"C:\\"), r"C:\");
	}

	/// A UNC root is server *and* share. Trimming the trailing separator off a
	/// bare `\\server\share`, or collapsing the leading pair, names nothing.
	#[test]
	fn a_unc_root_survives_normalisation() {
		assert_eq!(key(r"\\server\share"), r"\\SERVER\SHARE");
		assert_eq!(key(r"\\server\share\"), r"\\SERVER\SHARE");
		assert_eq!(key(r"\\server\share\notes.copper"), r"\\SERVER\SHARE\NOTES.COPPER");
	}

	#[test]
	fn the_verbatim_form_normalises_to_the_ordinary_one() {
		assert!(same(r"\\?\C:\x\a.copper", r"C:\x\a.copper"));
		assert!(same(r"\\?\UNC\srv\share\a.copper", r"\\srv\share\a.copper"));
	}

	/// A6: a hand-edited entry carrying the design's example prefix must dedupe
	/// against the same file opened normally, or it appears twice in the switcher.
	#[test]
	fn a_leading_appdata_variable_expands() {
		let Ok(appdata) = std::env::var("APPDATA") else {
			return;
		};
		let expanded = format!("{}\\Copper\\spaces\\p.copper", appdata.trim_end_matches('\\'));
		assert!(same(r"%APPDATA%\Copper\spaces\p.copper", &expanded));
		assert!(same(r"%appdata%\Copper\spaces\p.copper", &expanded));
	}

	/// `%` is legal in a filename, so a general `%VAR%` pass would mangle this.
	#[test]
	fn percent_signs_elsewhere_are_left_alone() {
		assert_eq!(key(r"C:\notes\100%\done.copper"), r"C:\NOTES\100%\DONE.COPPER");
		assert_eq!(key(r"C:\%APPDATA%\x.copper"), r"C:\%APPDATA%\X.COPPER");
	}

	#[test]
	fn a_relative_path_resolves_against_the_working_directory() {
		let cwd = std::env::current_dir().unwrap();
		assert_eq!(key("a.copper"), comparison_key(&cwd.join("a.copper")));
		assert_eq!(key(r".\sub\a.copper"), comparison_key(&cwd.join("sub").join("a.copper")));
	}

	/// Drive-relative and root-relative both have a root of their own, so neither
	/// may be joined onto the working directory.
	#[test]
	fn rooted_shapes_are_recognised() {
		assert!(is_rooted(Path::new(r"C:\x")));
		assert!(is_rooted(Path::new("C:x")));
		assert!(is_rooted(Path::new(r"\x")));
		assert!(is_rooted(Path::new(r"\\server\share\x")));
		assert!(!is_rooted(Path::new("x")));
		assert!(!is_rooted(Path::new(r".\x")));
	}

	#[test]
	fn a_drive_relative_path_resolves_against_that_drive() {
		// Whatever the per-drive directory is, both spellings must agree — which is
		// the property that matters, since the value is process state.
		assert_eq!(key("Q:sub\\a.copper"), key("q:SUB/A.COPPER"));
		// And it is not the process working directory unless the drives match.
		assert!(key("Q:a.copper").starts_with("Q:\\"));
	}

	#[test]
	fn display_strips_both_verbatim_shapes() {
		assert_eq!(display_path(Path::new(r"\\?\C:\x\a.copper")), r"C:\x\a.copper");
		assert_eq!(
			display_path(Path::new(r"\\?\UNC\srv\share\a.copper")),
			r"\\srv\share\a.copper"
		);
		assert_eq!(display_path(Path::new(r"C:\x\a.copper")), r"C:\x\a.copper");
	}

	#[test]
	fn drive_letters_are_read_from_the_prefix_only() {
		assert_eq!(drive_letter(Path::new(r"c:\x")), Some('C'));
		assert_eq!(drive_letter(Path::new(r"\\?\D:\x")), Some('D'));
		assert_eq!(drive_letter(Path::new(r"\\server\share\x")), None);
		assert_eq!(drive_letter(Path::new("x")), None);
	}
}
