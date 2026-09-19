//! Shared `npx` gate for the tests that run a real `napi build`.
//!
//! Every such test invokes `npx --yes -p @napi-rs/cli@<version> ...`, and `npx` installs the
//! package into a shared cache directory keyed by the package spec on first use. Two tests that
//! reach that first use concurrently -- which `cargo test`'s default thread pool makes routine --
//! both install into the same directory, and the loser can observe the winner's half-written
//! `node_modules`: `Test (ubuntu-latest)` on 2026-09-18 failed both napi fixtures with
//! `ERR_MODULE_NOT_FOUND ... fast-string-truncated-width` under `~/.npm/_npx/`. The
//! [`OnceLock`] here performs that first install exactly once, before any test's own `npx` runs,
//! so the build commands only ever find an already-populated cache. ~keep
//!
//! [`OnceLock`]: std::sync::OnceLock

use crate::core::template_versions as tv;

/// Whether `npx` runs, not merely resolves, and `@napi-rs/cli` is installed in its cache.
///
/// A version-manager shim (e.g. nvm) spawns fine then exits non-zero, so a PATH-only check would
/// leave the callers' skip unreachable and fire their asserts everywhere Node is absent. ~keep
pub(super) fn napi_cli_is_runnable() -> bool {
    static RUNNABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *RUNNABLE.get_or_init(|| npx_is_runnable() && warm_napi_cli_cache())
}

fn npx_is_runnable() -> bool {
    std::process::Command::new("npx")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Populate the `npx` cache for the exact package spec the build command emits, serialized
/// through the caller's [`std::sync::OnceLock`]. A failure here (no network on a cold cache, a
/// broken registry) is reported as "not runnable" so the callers skip, matching the convention
/// for tests that depend on an external toolchain.
fn warm_napi_cli_cache() -> bool {
    std::process::Command::new("npx")
        .args([
            "--yes",
            "-p",
            &format!("@napi-rs/cli@{}", tv::npm::NAPI_RS_CLI_CRATE),
            "napi",
            "--version",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
