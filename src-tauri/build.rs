/// The environment variable that turns the release manifest on.
///
/// Named rather than inferred from the profile alone because the condition the
/// manifest actually depends on is "this build will be Authenticode-signed", and
/// a build script has no way to see that: the certificate thumbprint reaches the
/// bundler through the Tauri CLI's config, long after this has run. Making it an
/// explicit opt-in keeps the two halves of a release build — the manifest and the
/// signature — set by the same documented step, and keeps a plain `cargo build
/// --release` or `tauri build` producing a binary that starts.
const UIACCESS_ENV: &str = "COPPER_UIACCESS";

fn main() {
	if let Err(error) = tauri_build::try_build(
		tauri_build::Attributes::new().windows_attributes(windows_attributes()),
	) {
		panic!("error found during tauri-build: {error:#}");
	}
}

/// Picks the application manifest, which is the dev/release split in full.
///
/// `WindowsAttributes::new()` already carries tauri-build's stock manifest, so
/// the dev branch is the absence of a call rather than a second manifest that has
/// to be kept in step with upstream's. That matters: the release manifest is a
/// copy of upstream's plus one block, and only one of the two can drift.
///
/// An unsigned binary whose manifest declares `uiAccess="true"` does not start at
/// all — CreateProcess fails with error 740 — so this is not a capability that
/// degrades. Getting the condition wrong in the permissive direction produces an
/// app that cannot be launched, which is why both halves have to hold.
fn windows_attributes() -> tauri_build::WindowsAttributes {
	// Toggling the variable has to invalidate the compiled resource, or the first
	// release build after a dev build silently ships the dev manifest.
	println!("cargo:rerun-if-env-changed={UIACCESS_ENV}");
	println!("cargo:rerun-if-changed=windows-app-manifest-uiaccess.xml");

	let attributes = tauri_build::WindowsAttributes::new();
	if !ui_access_requested() {
		return attributes;
	}

	attributes.app_manifest(include_str!("windows-app-manifest-uiaccess.xml"))
}

/// True only for a release profile that asked for it.
///
/// The profile check is not redundant with the variable. A shell that still has
/// `COPPER_UIACCESS` set from an earlier release build would otherwise hand the
/// manifest to the next `tauri dev`, and the symptom — a dev build that exits
/// before it draws anything — reads as a broken app rather than as a stale
/// variable.
fn ui_access_requested() -> bool {
	let opted_in = std::env::var(UIACCESS_ENV).is_ok_and(|value| value == "1");
	opted_in && std::env::var("PROFILE").is_ok_and(|profile| profile == "release")
}
