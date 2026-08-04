//! Embed the uiAccess manifest into the executable.
//!
//! Done through the MSVC linker directly rather than with a helper crate: the
//! whole point of this binary is that its manifest is exactly what we think it
//! is, and one linker flag is easier to audit than a build-dependency.
//!
//! Verify the result with:
//! ```text
//! mt.exe -inputresource:uiaccess-test.exe;#1 -out:extracted.manifest
//! ```

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("uiaccess.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());

    if std::env::var("CARGO_FEATURE_UIACCESS").is_err() {
        // No manifest at all. An unsigned binary that *does* carry
        // uiAccess="true" cannot be launched without elevation, so this is the
        // only shape a dev build can usefully take.
        println!("cargo:warning=built WITHOUT the uiAccess manifest (--no-default-features)");
        return;
    }

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
        // Without this, link.exe merges its own default manifest in as well.
        println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:NO");
    } else {
        println!(
            "cargo:warning=uiaccess-test needs the MSVC toolchain to embed its manifest; \
             the binary will build but uiAccess will be inactive"
        );
    }
}
