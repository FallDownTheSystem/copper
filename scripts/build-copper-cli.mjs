#!/usr/bin/env node
// Builds copper-cli and stages it where Tauri's `externalBin` bundling expects
// to find it. Runs as `build.beforeBundleCommand` — after the app binary is
// already compiled, before the NSIS bundler collects its inputs — so a cargo
// build for a workspace-sibling package cannot race the app's own build, and
// the staged file exists by the time bundling starts.
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const TARGET = "x86_64-pc-windows-msvc"; // the only target this project ships
const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

execFileSync("cargo", ["build", "--release", "-p", "copper-cli", "--target", TARGET], {
	cwd: repoRoot,
	stdio: "inherit",
});

// Ask cargo where it actually put the binary rather than assuming
// `src-tauri/target`. `.cargo/config.toml` pins that directory today, but
// CARGO_TARGET_DIR overrides it — and doc-release-process.md offers exactly that
// as a way to get a clean build. Hardcoding the path would then find a stale exe
// from an earlier build and stage it, silently shipping the wrong CLI. An
// explicit --target nests the output one level deeper, which is the TARGET
// segment below.
const metadata = JSON.parse(
	execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
		cwd: repoRoot,
		encoding: "utf8",
	}),
);
const builtExe = join(metadata.target_directory, TARGET, "release", "copper-cli.exe");
if (!existsSync(builtExe)) {
	throw new Error(
		`copper-cli built, but no executable at ${builtExe}. ` +
			`Check the [[bin]] name in copper-cli/Cargo.toml and the target-dir in .cargo/config.toml.`,
	);
}

// externalBin resolves "binaries/copper-cli" to this exact name; the suffix is
// the target triple, not a variable Tauri fills in for us.
const destDir = join(repoRoot, "src-tauri", "binaries");
mkdirSync(destDir, { recursive: true });
copyFileSync(builtExe, join(destDir, `copper-cli-${TARGET}.exe`));
