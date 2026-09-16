//! Regression coverage for pinning `poly` in the `generated-output-gate` CI job.
//!
//! Kept in its own file rather than growing `tests/generated_output_downstream_gate.rs`, which
//! is already over the `file-modularization` line cap: that file's purpose in this change is
//! unrelated (it hosts the wider generated-output gate), so it must not grow to carry an
//! assertion whose whole point is the poly pin. See `tests/file_size_baseline.txt`. ~keep
//!
//! Unlike the lanes in `generated_output_downstream_gate.rs`, this test needs no external
//! tooling -- it only reads `.github/workflows/ci.yml` as text -- so it runs as an ordinary,
//! non-`#[ignore]`d part of `cargo test --workspace`.

use std::path::PathBuf;

#[path = "workflow_job_block/support.rs"]
mod workflow_job_block_support;
use workflow_job_block_support::workflow_job_block;

/// The job whose steps must install poly via a pinned, checksum-verified action rather than an
/// unpinned Homebrew tap.
const GATE_JOB: &str = "generated-output-gate";

/// Length of a full git commit SHA, the only `uses:` ref shape that is immutable.
///
/// The property under test is *that* the action is pinned to a commit, not *which* commit:
/// naming one SHA here made the gate fail on every routine `Goldziher/poly` bump, and it did --
/// twice, masked both times because `cargo test` aborts after the first failing test target and
/// an unrelated failure in an earlier binary ran first. A gate that cries wolf on green changes
/// gets its constant bumped on reflex, which is not review. The poly release that actually runs
/// is pinned separately and exactly by [`PINNED_POLY_VERSION`]. ~keep
const COMMIT_SHA_LENGTH: usize = 40;

/// The `version:` value the action's `with:` block, and the "Verify downstream tooling"
/// step's own runtime check, must both agree on.
const PINNED_POLY_VERSION: &str = "v0.24.0";

/// Whether `block` has an actual `uses:` step line pinning `Goldziher/poly` to a commit SHA.
///
/// Deliberately line-scoped rather than a whole-block substring search: the surrounding
/// comments and the version-mismatch error message both mention `Goldziher/poly` refs in prose,
/// so a whole-block search would pass even if the `uses:` line itself were deleted or reverted
/// to the tag. Only a line whose trimmed text starts with `uses: Goldziher/poly@` counts, and
/// the ref that follows must be a full lowercase hex commit SHA -- `@v0` and `@v0.27.0` are
/// mutable and can be repointed, so they are not supply-chain pins. ~keep
fn uses_line_pins_poly_at_a_commit(block: &str) -> bool {
    const USES_PREFIX: &str = "uses: Goldziher/poly@";
    block.lines().any(|line| {
        let Some(rest) = line.trim_start().strip_prefix(USES_PREFIX) else {
            return false;
        };
        let git_ref = rest.split_whitespace().next().unwrap_or_default();
        git_ref.len() == COMMIT_SHA_LENGTH
            && git_ref
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    })
}

/// Whether `block` has an actual `with:` mapping line setting `version:` to `version`.
///
/// Line-scoped for the same reason as [`uses_line_pins_poly_at_sha`]: the step's own leading
/// comment says "`version: v0.22.0` in `with:` is kept as..." in prose, which a bare
/// `block.contains("version: v0.22.0")` would accept even with the real `with:` value deleted.
/// Only a non-comment line whose trimmed text is exactly `version: <version>` -- the shape a
/// YAML mapping value actually takes -- counts. ~keep
fn with_block_pins_poly_version(block: &str, version: &str) -> bool {
    let expected = format!("version: {version}");
    block
        .lines()
        .any(|line| !line.trim_start().starts_with('#') && line.trim() == expected)
}

/// Whether `block` has a non-comment line that actually invokes the hidden-subcommand probe,
/// rather than merely describing it.
///
/// Today's leading comment happens to line-wrap `poly hooks check` and `--added-large-files
/// --help` onto separate lines, so a bare `block.contains(needle)` is non-vacuous only by that
/// accident -- reflowing the comment onto one line would silently make it pass with the real
/// `run:` invocation deleted. Excluding comment lines (`#`-prefixed after trimming) closes that
/// regardless of how the prose is wrapped. ~keep
fn run_step_checks_hidden_subcommand(block: &str) -> bool {
    const NEEDLE: &str = "poly hooks check --added-large-files --help";
    block
        .lines()
        .any(|line| !line.trim_start().starts_with('#') && line.contains(NEEDLE))
}

#[test]
fn ci_workflow_pins_poly_in_the_generated_output_gate() {
    let workflow_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/ci.yml");
    let workflow = std::fs::read_to_string(&workflow_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", workflow_path.display()));

    let block = workflow_job_block(&workflow, GATE_JOB)
        .unwrap_or_else(|| panic!("{} has no `{GATE_JOB}` job", workflow_path.display()));

    assert!(
        uses_line_pins_poly_at_a_commit(&block),
        "the `{GATE_JOB}` job in {} must have a `uses: Goldziher/poly@<commit sha>` step, not \
         just prose mentioning a ref -- a mutable `@v0`/`@v0.27.0` tag pin, or a step deleted \
         outright, would not satisfy this:\n\
         --- job block as parsed ---\n{block}",
        workflow_path.display()
    );

    assert!(
        with_block_pins_poly_version(&block, PINNED_POLY_VERSION),
        "the `{GATE_JOB}` job in {} must have a `with: version: {PINNED_POLY_VERSION}` step \
         input, not just prose mentioning the version -- poly.toml's added-large-files hook \
         depends on poly's hidden `hooks check` subcommand, so an unpinned or `latest` install \
         can silently break that guard on a poly upgrade:\n\
         --- job block as parsed ---\n{block}",
        workflow_path.display()
    );

    assert!(
        run_step_checks_hidden_subcommand(&block),
        "the `{GATE_JOB}` job in {} must actually run `poly hooks check --added-large-files \
         --help`, not just describe it in a comment -- a poly release that renames/removes the \
         hidden subcommand must fail this CI job, not fail silently:\n\
         --- job block as parsed ---\n{block}",
        workflow_path.display()
    );
}

/// [`uses_line_pins_poly_at_a_commit`] must actually discriminate: it should fail when the real
/// `uses:` line is missing, reverted to a tag, or only described in a comment. Without this, a
/// future edit could reintroduce the exact vacuity this file exists to close, with nothing here
/// to catch it. ~keep
#[test]
fn uses_line_pins_poly_at_a_commit_rejects_comment_only_and_reverted_pins() {
    let pinned_uses =
        "      - name: Install poly\n        uses: Goldziher/poly@fed55c3355480f0d1c23cb6084395e66bbb1cdc8 # v0.22.0\n";
    assert!(
        uses_line_pins_poly_at_a_commit(pinned_uses),
        "a real `uses:` line pinning a commit SHA must be accepted"
    );
    let uses_comment_only =
        "      # Goldziher/poly@v0 currently resolves to fed55c3355480f0d1c23cb6084395e66bbb1cdc8\n";
    assert!(
        !uses_line_pins_poly_at_a_commit(uses_comment_only),
        "prose mentioning the tag and SHA, with no `uses:` line, must not satisfy the check"
    );
    let uses_tag_pin = "      - name: Install poly\n        uses: Goldziher/poly@v0\n";
    assert!(
        !uses_line_pins_poly_at_a_commit(uses_tag_pin),
        "reverting to the mutable `@v0` tag must not satisfy the commit-pin check"
    );
    let uses_release_tag_pin = "      - name: Install poly\n        uses: Goldziher/poly@v0.27.0\n";
    assert!(
        !uses_line_pins_poly_at_a_commit(uses_release_tag_pin),
        "an exact-looking but still mutable release tag must not satisfy the commit-pin check"
    );
    let uses_abbreviated_sha = "      - name: Install poly\n        uses: Goldziher/poly@fed55c3\n";
    assert!(
        !uses_line_pins_poly_at_a_commit(uses_abbreviated_sha),
        "an abbreviated SHA is ambiguous and must not satisfy the commit-pin check"
    );
    let uses_deleted = "      - name: Some other step\n        run: echo hi\n";
    assert!(
        !uses_line_pins_poly_at_a_commit(uses_deleted),
        "deleting the `uses:` step outright must not satisfy the commit-pin check"
    );
}

/// [`with_block_pins_poly_version`] must actually discriminate: it should fail when the real
/// `with: version:` line is missing or only described in a comment. ~keep
#[test]
fn with_block_pins_poly_version_rejects_comment_only_and_deleted_values() {
    let version = "v0.22.0";

    let pinned_with = "        with:\n          version: v0.22.0\n";
    assert!(
        with_block_pins_poly_version(pinned_with, version),
        "a real `with: version:` line must be accepted"
    );
    let with_comment_only = "        # the SHA is the real supply-chain pin; `version: v0.22.0` in `with:` is kept\n";
    assert!(
        !with_block_pins_poly_version(with_comment_only, version),
        "prose that merely mentions `version: v0.22.0` in a comment must not satisfy the check"
    );
    let with_deleted = "        with:\n          some-other-key: true\n";
    assert!(
        !with_block_pins_poly_version(with_deleted, version),
        "deleting the `version:` input outright must not satisfy the check"
    );
}

/// [`run_step_checks_hidden_subcommand`] must actually discriminate: it should fail when the
/// real invocation is missing or only described in a comment -- including a comment reflowed
/// onto one line, which is the exact vacuity this helper exists to close. ~keep
#[test]
fn run_step_checks_hidden_subcommand_rejects_comment_only_and_reflowed_prose() {
    let real_run_line = "          if ! poly hooks check --added-large-files --help >/dev/null 2>&1; then\n";
    assert!(
        run_step_checks_hidden_subcommand(real_run_line),
        "a real invocation of the hidden-subcommand probe must be accepted"
    );
    let wrapped_comment = "      # rather than a bare `poly --version`: (2) `poly hooks check\n      # --added-large-files --help` must still resolve\n";
    assert!(
        !run_step_checks_hidden_subcommand(wrapped_comment),
        "a line-wrapped comment describing the probe must not satisfy the check"
    );
    let reflowed_comment = "      # (2) `poly hooks check --added-large-files --help` must still resolve\n";
    assert!(
        !run_step_checks_hidden_subcommand(reflowed_comment),
        "reflowing that same comment onto one line must still not satisfy the check -- this is \
         the exact vacuity this helper exists to close"
    );
    let run_deleted = "          echo \"poly hooks check is not invoked here\"\n";
    assert!(
        !run_step_checks_hidden_subcommand(run_deleted),
        "deleting the real invocation outright must not satisfy the check"
    );
}
