use std::path::Path;
use tracing::debug;

/// Normalize content the same way `write_files` does before hashing.
///
/// Rust files go through rustfmt for canonical formatting, then through
/// `normalize_whitespace` so trailing-whitespace and trailing-newline rules
/// hold even when rustfmt could not parse the file (e.g. cextendr `lib.rs`
/// with non-standard `parameter: T = "default"` syntax that rustfmt rejects;
/// without the second pass, the raw codegen output retains trailing
/// whitespace on blank lines, and prek's `trailing-whitespace` hook then
/// rewrites the file post-finalisation, breaking `alef verify`).
///
/// Non-rust files skip rustfmt and go straight to whitespace normalization.
///
/// Before that whitespace pass runs, every *content-bearing* line (one that is not already
/// blank) is checked for trailing whitespace and rejected loudly if it has any --
/// see [`reject_trailing_whitespace_on_content_lines`] for why a silent trim is unsafe here.
/// Blank lines are exempt and keep being silently collapsed: layout noise, not data.
pub fn normalize_content(path: &Path, content: &str) -> anyhow::Result<String> {
    let pre = if path.extension().is_some_and(|ext| ext == "rs") {
        format_rust_content(path, content)
    } else {
        content.to_string()
    };
    reject_trailing_whitespace_on_content_lines(path, &pre)?;
    let is_markdown = path.extension().is_some_and(|ext| ext == "md");
    Ok(normalize_whitespace_with_policy(&pre, is_markdown))
}

/// The one deliberate exception to [`normalize_content`]'s loud-failure policy: content that
/// is not alef's own codegen output at all, so there is no emitter to hold accountable for
/// choosing a safe literal form, and no fixture-driven "value" the whitespace could belong to.
///
/// Used solely by [`crate::backends::swift::gen_bindings::bridge_artifacts::write_materialized_files`]
/// for `RustBridgeC.h`/`*.swift`, which are read straight off swift-bridge's own external build
/// output (a third-party tool's C/Swift source, not an alef template or fixture assertion) and
/// written to disk with no other normalization pass in between. Trailing whitespace there (e.g.
/// swift-bridge's own `#include <stdbool.h> `) is unconditionally incidental formatting noise
/// from a tool alef does not control -- there is no raw/verbatim string-literal concept in a C
/// header at all, so the ambiguity [`reject_trailing_whitespace_on_content_lines`] exists to
/// catch cannot arise here. Silently trimming it (as this whole module did everywhere, before
/// alef-task #557) remains correct for this one path; failing generation over a third-party
/// tool's own harmless formatting would be a false-positive with no fix available to any
/// emitter. ~keep
pub fn normalize_foreign_tool_content(path: &Path, content: &str) -> String {
    let is_markdown = path.extension().is_some_and(|ext| ext == "md");
    normalize_whitespace_with_policy(content, is_markdown)
}

/// Fail loudly when a non-blank line of generated content carries trailing whitespace.
///
/// The old behaviour here was a silent trim inside [`normalize_whitespace_with_policy`], on
/// the theory that trailing whitespace in generated output is always incidental layout noise
/// that prek's `trailing-whitespace` hook would strip anyway -- so alef might as well strip it
/// first and keep `alef verify` stable. That theory is wrong for one shape of content: a
/// multi-line raw/verbatim string literal (a Go backtick literal, a Rust raw string, ...)
/// where an emitter chose to reproduce a value byte-for-byte, real newlines included. When
/// that value carries whitespace immediately before a newline -- a Markdown two-space hard
/// line break is exactly this shape -- the trailing whitespace is not layout, it is the
/// *value being asserted*. Silently trimming it rewrites the literal to something nobody wrote,
/// and nothing here reports that it happened: a downstream repo shipped a generated Go e2e
/// suite whose assertions read `[Alpha\n](url)Beta` where every fixture said
/// `[Alpha␣␣\n](url)Beta`. What eventually surfaced it was those tests failing against correct
/// output, one full CI matrix later -- not this function, which had no idea it had changed a
/// value rather than tidied a line.
///
/// The fix belongs in the emitter: a backend able to reach this shape must decide, per value,
/// whether a raw/verbatim literal is safe to emit -- and fall back to an escaped/quoted form
/// (`\n`, `\t`, ...) when it is not. See `go_needs_quoted` and `rust_needs_quoted` in
/// `src/e2e/escape.rs` for the established pattern. This check exists so a backend that skips
/// that step fails generation immediately, at the exact line, instead of shipping silently
/// corrupted output.
///
/// Blank lines (whitespace-only, or empty) are intentionally exempt -- see the doc comment on
/// [`normalize_whitespace_with_policy`] for why those still need silent collapsing, and the
/// `test_normalize_content_strips_trailing_whitespace_when_rustfmt_fails` test that pins it. ~keep
fn reject_trailing_whitespace_on_content_lines(path: &Path, content: &str) -> anyhow::Result<()> {
    for (zero_based_line, line) in content.lines().enumerate() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.len() == line.len() {
            continue;
        }
        let trailing = &line[trimmed.len()..];
        let visible_trailing: String = trailing.chars().map(|c| if c == '\t' { '⇥' } else { '·' }).collect();
        anyhow::bail!(
            "{}: line {} has {} byte(s) of trailing whitespace on a non-blank line: \
             {trimmed}{visible_trailing}\n\
             This would be silently rewritten (data loss, not formatting) by trimming it -- if \
             the trailing whitespace is meant to be part of a string literal's value (e.g. a \
             Markdown hard line break), the emitter must escape it into an interpreted/quoted \
             literal instead of emitting it as raw/verbatim text. See `go_needs_quoted` / \
             `rust_needs_quoted` in src/e2e/escape.rs for the established pattern.",
            path.display(),
            zero_based_line + 1,
            trailing.len(),
        );
    }
    Ok(())
}

/// Normalize whitespace for comparison: strip trailing whitespace per line,
/// collapse runs of 3+ blank lines to 2 (1 for markdown), and ensure a single
/// trailing newline.
///
/// Markdown files get an aggressive 1-blank-line cap because the canonical
/// downstream pre-commit pipeline runs `rumdl-fmt` after every commit,
/// and rumdl's MD012 rule collapses any multi-blank run to a single blank.
/// Without the matching cap inside alef, `alef all` output (which goes
/// through pre-commit `rumdl-fmt` before being committed) diverges from the
/// cold `alef readme` output (which does not invoke any markdown formatter),
/// and CI's `Validate READMEs` step — which runs `alef readme` cold and
/// diffs against the committed file — fails on every regen with the
/// noisy "extra blank line between `##` headings" diff. Capping at 1
/// inside alef itself produces rumdl-clean output natively, so cold and
/// hot paths converge and CI is stable.
///
/// Empty input stays empty — the canonical pre-commit `end-of-file-fixer`
/// hook truncates whitespace-only files (including a lone `"\n"`) to zero
/// bytes, so re-inflating empty content to `"\n"` here would create an
/// infinite emit/format ping-pong (e.g. for `.gitkeep` placeholders).
pub(super) fn normalize_whitespace(content: &str) -> String {
    normalize_whitespace_with_policy(content, false)
}

fn normalize_whitespace_with_policy(content: &str, is_markdown: bool) -> String {
    if content.is_empty() {
        return String::new();
    }
    let max_blanks: usize = if is_markdown { 1 } else { 2 };
    let mut result = String::with_capacity(content.len());
    let mut blank_count = 0usize;
    for line in content.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= max_blanks {
                result.push('\n');
            }
        } else {
            blank_count = 0;
            result.push_str(trimmed);
            result.push('\n');
        }
    }
    while result.ends_with("\n\n") {
        result.pop();
    }
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Walk up from `path` to find the nearest `Cargo.toml` and read its
/// `[package] edition = "YYYY"` value.  Returns `"2024"` if no `Cargo.toml`
/// is found or the edition field is absent.
pub(super) fn detect_crate_edition(path: &Path) -> String {
    let start = if path.is_dir() {
        path
    } else {
        match path.parent() {
            Some(p) => p,
            None => return "2024".to_string(),
        }
    };

    let mut current = start;
    loop {
        let candidate = current.join("Cargo.toml");
        if candidate.is_file() {
            if let Ok(text) = std::fs::read_to_string(&candidate)
                && let Some(edition) = parse_package_edition(&text)
            {
                return edition;
            }
            return "2024".to_string();
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    "2024".to_string()
}

/// Parse the `edition = "YYYY"` value from the `[package]` section of a
/// `Cargo.toml` string.  Returns `None` if not found.
pub(super) fn parse_package_edition(toml_text: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml_text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("edition") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"');
                if value.len() == 4 && value.chars().all(|c| c.is_ascii_digit()) {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Format a Rust source string by piping through `rustfmt`.
///
/// The edition is detected from the nearest `Cargo.toml` above `path`,
/// defaulting to `"2024"` when none is found.  `rustfmt` also discovers the
/// project's `rustfmt.toml` from the working directory.
///
/// Returns the formatted content on success, or the original content if
/// rustfmt is unavailable or fails (best-effort).
pub fn format_rust_content(path: &Path, content: &str) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let edition = detect_crate_edition(path);
    let config_dir = std::env::current_dir().unwrap_or_default();

    let mut child = match Command::new("rustfmt")
        .arg("--edition")
        .arg(&edition)
        .arg("--config-path")
        .arg(&config_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            debug!("rustfmt not available: {e}");
            return content.to_string();
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(content.as_bytes());
    }

    match child.wait_with_output() {
        Ok(output) if output.status.success() => {
            String::from_utf8(output.stdout).unwrap_or_else(|_| content.to_string())
        }
        Ok(output) => {
            debug!("rustfmt failed: {}", String::from_utf8_lossy(&output.stderr));
            content.to_string()
        }
        Err(e) => {
            debug!("rustfmt process error: {e}");
            content.to_string()
        }
    }
}
