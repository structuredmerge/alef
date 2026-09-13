//! The one place that decides where snippet validation writes its scratch, and the one guard that
//! removes it again.
//!
//! Before this module existed, every language runner picked its own scratch destination with a
//! bare `tempfile_in`/`tempdir_in` against whatever directory happened to be in scope, and the
//! results disagreed in ways that were invisible until a consumer ran `git status`: some scratch
//! landed inside an ignored cache directory, some landed loose in a tracked package source
//! directory under an ignored *name*, and some landed loose under a name matching no ignore rule
//! at all, where a consumer could stage it by accident. All three are the same defect — the
//! destination was per-runner behaviour rather than one piece of tree configuration — so all three
//! are fixed in one place rather than papered over with an ignore rule. An ignore rule would hide
//! the leak while leaving the file on disk, and a leaked `snippet.go` on disk is indistinguishable
//! from an orphaned source file to any tool that scans by marker and path. ~keep

use crate::snippets::error::{Error, Result};
use crate::snippets::session::ValidationSession;
use crate::snippets::types::Language;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The in-tree root every per-snippet scratch directory nests under, relative to the base
/// [`scratch_root`] picks.
///
/// `**/.alef/` is already ignored everywhere alef generates, so *nesting under this path* is what
/// actually keeps scratch out of a consumer's index. The `.alef-snippet-` prefix below is only a
/// marker for [`purge_stale_scratch_root`] and for forensics — it never hid anything, which is
/// precisely why the pre-fix `packages/go/.alef-snippet-*/` leak was still sitting in the tree. ~keep
pub const SNIPPET_SCRATCH_ROOT: &str = ".alef/snippets/tmp";

/// Marker prefix on every scratch directory alef allocates.
const SCRATCH_PREFIX: &str = ".alef-snippet-";

/// How long past a run's own per-snippet timeout an entry must have gone untouched before
/// [`purge_stale_scratch_root`] treats it as abandoned rather than as a concurrently running
/// validation's live scratch. Two alef processes can legitimately share one working directory, and
/// without this window the sweep would delete the other one's work in progress. ~keep
///
/// Shared with `session::toolchain_cache`, which reclaims fallen-out-of-use compiler caches under
/// the identical hazard: one margin, one rationale, rather than two numbers that can drift. ~keep
pub(super) const ABANDONED_GRACE_SECS: u64 = 60;

/// A killed child can still be exiting — and still holding or recreating entries — when its
/// scratch guard drops, so a single `remove_dir_all` can lose the race and return `ENOTEMPTY`.
/// `tempfile::TempDir` discards exactly that error, which is how scratch survived a clean exit.
/// Retry a bounded number of times, then say so out loud rather than silently leaving litter. ~keep
const REMOVAL_ATTEMPTS: u32 = 3;
const REMOVAL_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Whether a language's toolchain resolves the snippet's own package by walking *up from the
/// scratch file*, rather than from the process working directory alef sets.
///
/// `go build <file>` locates the enclosing module from the named file's directory, and `dart
/// analyze` resolves `package:` imports from the analyzed file's directory upwards; for both,
/// scratch placed outside the manifest's directory fails to resolve the local package no matter
/// what `current_dir` says. Every other language resolves from the working directory or from
/// absolute classpath/include entries and must *not* be moved under the manifest — Rust is the
/// counter-example that makes this a table rather than a blanket rule: alef's own Rust session
/// uses `cwd = "."` with `manifest = "crates/<crate>/Cargo.toml"`, and a scratch `Cargo.toml`
/// nested inside that crate directory would be pulled into the surrounding cargo workspace and
/// break the build it is meant to validate.
///
/// This is a table of two entries with a stated reason each, in one function, deliberately shaped
/// after `session::keeps_scratch_outside_working_directory`. Adding a language here is a
/// considered change; a language runner choosing its own destination is the bug. ~keep
const fn resolves_package_from_the_scratch_file(language: Language) -> bool {
    matches!(language, Language::Dart | Language::Go)
}

/// The single decision: the directory under which every per-snippet scratch entry for a session is
/// allocated.
///
/// No language runner may compute this itself. Runners reach it through
/// [`ScratchDir::for_session`] (or [`ValidationSession::scratch_root`]), and session preparation
/// reaches the identical path through this function before a `ValidationSession` exists, so the
/// sweep and the allocator can never disagree about where scratch lives. ~keep
#[must_use]
pub fn scratch_root(language: Language, working_directory: &Path, manifest: Option<&Path>) -> PathBuf {
    let base = manifest
        .filter(|_| resolves_package_from_the_scratch_file(language))
        .and_then(Path::parent)
        .unwrap_or(working_directory);
    base.join(SNIPPET_SCRATCH_ROOT)
}

/// An owned, uniquely-named scratch directory that removes itself on every exit path.
///
/// Cleanup is a `Drop` guard rather than an explicit call before each `return` because the
/// validators it serves return from many places: `?` on a spawn failure, `?` on a timeout, an
/// early `return Ok((Fail, _))` when the toolchain rejects the snippet, and the success path. The
/// pre-fix code cleaned up on some of those and not others, which is exactly why scratch survived
/// a clean exit — a guard makes the set of exit paths irrelevant instead of something each runner
/// has to enumerate correctly. ~keep
#[derive(Debug)]
pub struct ScratchDir {
    inner: tempfile::TempDir,
}

impl ScratchDir {
    /// Allocates scratch for `session` under the destination [`scratch_root`] chose for it.
    ///
    /// # Errors
    ///
    /// Returns an error when the scratch root cannot be created or a unique directory cannot be
    /// allocated inside it.
    pub fn for_session(session: &ValidationSession) -> Result<Self> {
        Self::in_root(&session.scratch_root())
    }

    /// Allocates scratch under the OS temporary directory, for validation that has no session and
    /// therefore no consumer tree to keep clean.
    ///
    /// # Errors
    ///
    /// Returns an error when the OS temporary directory cannot be written to.
    pub fn isolated() -> Result<Self> {
        let inner = tempfile::TempDir::new()
            .map_err(|error| Error::Other(format!("allocating isolated snippet scratch: {error}")))?;
        Ok(Self { inner })
    }

    /// Allocates scratch nested inside `root` -- a real, on-disk project root a caller has already
    /// resolved by other means, not a configured session -- rather than the OS temporary
    /// directory [`isolated`](Self::isolated) uses. Reuses the exact `.alef/snippets/tmp`
    /// convention and cleanup machinery session-backed scratch already relies on
    /// ([`SNIPPET_SCRATCH_ROOT`], [`purge_stale_scratch_root`]) instead of inventing a second
    /// in-tree scratch location, and sweeps that root before allocating: unlike a session's own
    /// root, nothing else ever calls [`purge_stale_scratch_root`] for this path (there is no
    /// `SessionSpec` to run `prepare_sessions_isolated`'s phase-two sweep), so a crashed run's
    /// leftovers here would otherwise sit in the consumer's tree forever. See
    /// `node_project_root::resolve_isolated_scratch` (`src/snippets/validators/
    /// node_project_root.rs`) for the first and, as of this writing, only caller. ~keep
    ///
    /// # Errors
    ///
    /// Returns an error when the scratch root cannot be swept, created, or a unique directory
    /// cannot be allocated inside it.
    pub fn rooted(root: &Path, timeout_secs: u64) -> Result<Self> {
        let scratch_root = root.join(SNIPPET_SCRATCH_ROOT);
        purge_stale_scratch_root(&scratch_root, timeout_secs)?;
        Self::in_root(&scratch_root)
    }

    fn in_root(root: &Path) -> Result<Self> {
        // ~keep Tag the scratch root's PARENT, never the scratch root itself. `purge_scratch_root_entries`
        // deletes every entry under the scratch root that is older than the abandonment threshold, and a
        // `CACHEDIR.TAG` written here is exactly such an entry -- it would be written, aged out, swept, and
        // rewritten on every run. The parent is an ordinary cache directory the sweep never enumerates, and
        // a tag there already excludes everything below it, because a conforming reader stops recursing at
        // the first tag it meets. `ensure_project_cache_dir` additionally tags the `.alef/` root above the
        // parent, which is the directory such a reader actually meets first and probes -- it does not accept
        // a tag found further down, so the parent's tag does not answer for it.
        match root.parent() {
            Some(parent) => crate::core::cache_dir::ensure_project_cache_dir(parent)
                .and_then(|()| std::fs::create_dir_all(root))
                .map_err(|error| Error::Other(format!("creating snippet scratch root {}: {error}", root.display())))?,
            None => std::fs::create_dir_all(root)
                .map_err(|error| Error::Other(format!("creating snippet scratch root {}: {error}", root.display())))?,
        }
        let inner = tempfile::Builder::new()
            .prefix(SCRATCH_PREFIX)
            .tempdir_in(root)
            .map_err(|error| Error::Other(format!("allocating snippet scratch in {}: {error}", root.display())))?;
        Ok(Self { inner })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.inner.path()
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let path = self.inner.path().to_path_buf();
        for attempt in 1..=REMOVAL_ATTEMPTS {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(error) if attempt == REMOVAL_ATTEMPTS => {
                    tracing::warn!(
                        scratch = %path.display(),
                        error = %error,
                        attempts = REMOVAL_ATTEMPTS,
                        "snippet scratch directory survived cleanup"
                    );
                    return;
                }
                Err(_) => std::thread::sleep(REMOVAL_RETRY_DELAY),
            }
        }
    }
}

/// Removes entries abandoned in a scratch root by a run that never got to drop its guards — a
/// `SIGINT`, a crash, or an orphaned grandchild that recreated files after the guard swept.
///
/// This is the counterpart the `Drop` guard cannot be: no in-process guard survives its own
/// process being killed, so without a sweep a single `Ctrl-C` leaves scratch in the consumer's
/// tree forever. It is deliberately narrow. It reads exactly one directory level of a path that
/// always ends in `.alef/snippets/tmp`, never recurses out of it, and never inspects a consumer's
/// own files — alef is the only writer beneath that root, which is what makes removing entries
/// there by age (rather than by name) safe, and what stops this from becoming another
/// scan-by-marker delete gate. ~keep
///
/// # Errors
///
/// Returns an error when the root exists but cannot be read, or an abandoned entry cannot be
/// removed.
pub(crate) fn purge_stale_scratch_root(root: &Path, timeout_secs: u64) -> Result<()> {
    purge_scratch_root_entries(
        root,
        Duration::from_secs(timeout_secs.saturating_add(ABANDONED_GRACE_SECS)),
    )
}

fn purge_scratch_root_entries(root: &Path, abandoned_after: Duration) -> Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(Error::Other(format!(
                "reading snippet scratch root {}: {error}",
                root.display()
            )));
        }
    };
    for entry in entries.flatten() {
        if !is_abandoned(&entry, abandoned_after) {
            continue;
        }
        let path = entry.path();
        let outcome = if entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(error) = outcome
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(Error::Other(format!(
                "removing abandoned snippet scratch {}: {error}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn is_abandoned(entry: &std::fs::DirEntry, abandoned_after: Duration) -> bool {
    entry
        .metadata()
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age >= abandoned_after))
}

#[cfg(test)]
mod tests {
    use super::{SNIPPET_SCRATCH_ROOT, ScratchDir, purge_stale_scratch_root, scratch_root};
    use crate::snippets::session::ValidationSession;
    use crate::snippets::types::Language;
    use crate::snippets::validators::ValidatorRegistry;
    use std::path::{Path, PathBuf};

    fn session(language: Language, working_directory: PathBuf, manifest: Option<PathBuf>) -> ValidationSession {
        ValidationSession {
            language,
            working_directory,
            manifest,
            fingerprint: "scratch-fixture".into(),
            env: std::collections::BTreeMap::new(),
            include_paths: Vec::new(),
            rust_features: Vec::new(),
            rust_dependencies: std::collections::BTreeMap::new(),
        }
    }

    /// The axis that matters is *every* language, not the handful that were observed leaking: the
    /// defect was that runners disagreed, so a fix verified against two of them proves nothing
    /// about the twenty that were never read. The language list is taken from the validator
    /// registry rather than written out here, so a newly registered language joins this assertion
    /// without anyone remembering to add it. ~keep
    #[test]
    fn every_registered_language_resolves_scratch_under_the_cache_root() {
        let languages = ValidatorRegistry::default().languages();
        assert!(
            languages.len() > 20,
            "the registry should carry every supported language, got {}",
            languages.len()
        );
        let working_directory = Path::new("/workspace/packages/example");
        let manifest_path = Path::new("/workspace/packages/example/nested/manifest.toml");

        for language in languages {
            for manifest in [None, Some(manifest_path)] {
                let root = scratch_root(language, working_directory, manifest);
                assert!(
                    root.ends_with(SNIPPET_SCRATCH_ROOT),
                    "{language} scratch must nest under {SNIPPET_SCRATCH_ROOT}, got {}",
                    root.display()
                );
                assert_ne!(
                    root, working_directory,
                    "{language} scratch must never be the working directory itself"
                );
                let inside_the_tree = root.starts_with(working_directory)
                    || manifest
                        .and_then(Path::parent)
                        .is_some_and(|parent| root.starts_with(parent));
                assert!(
                    inside_the_tree,
                    "{language} scratch must stay inside the session's own tree, got {}",
                    root.display()
                );
            }
        }
    }

    /// Pins the two-entry exception table and, just as importantly, its complement. Go and Dart
    /// resolve the local package by walking up from the scratch file, so their scratch has to sit
    /// under the manifest's directory; Rust must not, because alef's own Rust session pairs
    /// `cwd = "."` with a manifest deep inside a cargo workspace. If this ever became a blanket
    /// "always use the manifest's directory" rule, Rust snippet validation would break. ~keep
    #[test]
    fn only_package_resolving_languages_move_scratch_under_the_manifest() {
        let working_directory = Path::new("/workspace");
        let manifest = Path::new("/workspace/packages/example/manifest.toml");

        for language in [Language::Go, Language::Dart] {
            assert_eq!(
                scratch_root(language, working_directory, Some(manifest)),
                Path::new("/workspace/packages/example").join(SNIPPET_SCRATCH_ROOT),
                "{language} resolves its package from the scratch file"
            );
        }
        for language in [Language::Rust, Language::C, Language::Python, Language::TypeScript] {
            assert_eq!(
                scratch_root(language, working_directory, Some(manifest)),
                working_directory.join(SNIPPET_SCRATCH_ROOT),
                "{language} must keep scratch under the working directory"
            );
        }
    }

    #[test]
    fn a_session_allocates_scratch_inside_its_own_root_and_not_directly_in_the_tree() {
        let directory = tempfile::tempdir().expect("working directory");
        let session = session(Language::Python, directory.path().to_path_buf(), None);

        let scratch = ScratchDir::for_session(&session).expect("scratch directory");

        assert_eq!(
            scratch.path().parent(),
            Some(directory.path().join(SNIPPET_SCRATCH_ROOT).as_path()),
            "scratch must nest under the cache root, not sit directly in the working directory"
        );
        let loose: Vec<_> = std::fs::read_dir(directory.path())
            .expect("read working directory")
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| name != ".alef")
            .collect();
        assert!(loose.is_empty(), "nothing may be written loose in the tree: {loose:?}");
    }

    /// `rooted` must nest under the same `.alef/snippets/tmp` convention `for_session` uses --
    /// this is what lets `tsc`'s own ancestor `node_modules/@types` scan reach a real project's
    /// installed types once the scratch directory it writes `tsconfig.json` into sits inside that
    /// project's own tree, rather than the OS temp directory `isolated` uses. ~keep
    #[test]
    fn rooted_nests_scratch_under_the_same_cache_root_a_session_would() {
        let root = tempfile::tempdir().expect("project root");

        let scratch = ScratchDir::rooted(root.path(), 5).expect("rooted scratch directory");

        assert_eq!(
            scratch.path().parent(),
            Some(root.path().join(SNIPPET_SCRATCH_ROOT).as_path()),
            "rooted scratch must nest under the cache root, not sit directly in `root`"
        );
    }

    /// Allocating scratch is the first thing that creates `.alef/` in most consumer trees, so it
    /// is also where `.alef/` has to become provable. A conforming reader probes for a
    /// `CACHEDIR.TAG` *directly inside* the directory it is looking at and does not accept one
    /// found higher up, so tagging only `in_root`'s chosen parent left the `.alef/` a backup or
    /// pruning tool actually meets carrying nothing. The scratch root itself must stay untagged
    /// for the reason `in_root` documents -- a tag written there is an entry the sweep ages out
    /// and deletes -- so that half is asserted here too. ~keep
    #[test]
    fn rooted_scratch_tags_the_alef_root_as_well_as_the_scratch_root_parent() {
        let root = tempfile::tempdir().expect("project root");

        let _scratch = ScratchDir::rooted(root.path(), 5).expect("rooted scratch directory");

        for directory in [root.path().join(".alef"), root.path().join(".alef/snippets")] {
            assert!(
                crate::core::cache_dir::is_tagged(&directory),
                "{} must carry a valid CACHEDIR.TAG",
                directory.display()
            );
        }
        assert!(
            !crate::core::cache_dir::is_tagged(&root.path().join(SNIPPET_SCRATCH_ROOT)),
            "the scratch root must stay untagged: its own sweep would age the tag out and delete it"
        );
    }

    /// A crashed run's leftover under a `rooted` root has no `SessionSpec` and therefore no
    /// `prepare_sessions_isolated` sweep to ever remove it -- `rooted` must sweep for itself, the
    /// same way session preparation sweeps its own root before activating. This test cannot age an
    /// entry past the sweep's grace window without a real (or backdated) mtime, so it proves the
    /// wiring the safe direction: the sweep genuinely runs against `root`'s own scratch cache
    /// (no error, no wrong-root panic) and still respects that grace window, the same way
    /// `the_sweep_spares_scratch_a_concurrent_run_may_still_be_using` proves it for a session's own
    /// root. Real removal of an aged-out entry is already covered there and in
    /// `the_sweep_removes_every_abandoned_shape_including_populated_directories`. ~keep
    #[test]
    fn rooted_sweeps_its_root_without_deleting_a_freshly_touched_concurrent_entry() {
        let root = tempfile::tempdir().expect("project root");
        let scratch_root = root.path().join(SNIPPET_SCRATCH_ROOT);
        let concurrent = scratch_root.join(".alef-snippet-concurrent");
        std::fs::create_dir_all(&concurrent).expect("concurrent scratch");

        let _scratch = ScratchDir::rooted(root.path(), 0).expect("rooted scratch directory");

        assert!(
            concurrent.exists(),
            "a freshly touched entry from a concurrent run must survive rooted's sweep"
        );
    }

    #[test]
    fn each_allocation_gets_a_distinct_directory() {
        let directory = tempfile::tempdir().expect("working directory");
        let session = session(Language::Python, directory.path().to_path_buf(), None);

        let first = ScratchDir::for_session(&session).expect("first scratch");
        let second = ScratchDir::for_session(&session).expect("second scratch");

        assert_ne!(first.path(), second.path());
    }

    /// The guard has to remove a *populated* directory, not just an empty one: every real runner
    /// writes a source file, and several point a toolchain cache at the same directory. ~keep
    #[test]
    fn dropping_the_guard_removes_the_directory_and_everything_in_it() {
        let directory = tempfile::tempdir().expect("working directory");
        let session = session(Language::Python, directory.path().to_path_buf(), None);
        let scratch = ScratchDir::for_session(&session).expect("scratch directory");
        let path = scratch.path().to_path_buf();
        std::fs::create_dir_all(path.join("cache/nested")).expect("nested cache");
        std::fs::write(path.join("snippet.py"), "value = 1\n").expect("scratch source");
        std::fs::write(path.join("cache/nested/artifact"), "artifact").expect("cached artifact");

        drop(scratch);

        assert!(!path.exists(), "the guard must remove its directory on drop");
        let root = directory.path().join(SNIPPET_SCRATCH_ROOT);
        let remaining = std::fs::read_dir(&root)
            .map(|entries| entries.flatten().count())
            .unwrap_or(0);
        assert_eq!(remaining, 0, "no scratch may survive under {}", root.display());
    }

    /// A guard cannot clean up after a process that was killed, so the sweep is the only thing
    /// standing between a `Ctrl-C` and permanent litter in a consumer's tree. It must reach both
    /// leaked shapes: the `.alef-snippet-*/` directory and the bare `.tmp<random>` file the
    /// script-based runners used to drop, which matched no ignore rule at all. ~keep
    #[test]
    fn the_sweep_removes_every_abandoned_shape_including_populated_directories() {
        let directory = tempfile::tempdir().expect("working directory");
        let root = directory.path().join(SNIPPET_SCRATCH_ROOT);
        std::fs::create_dir_all(&root).expect("scratch root");
        let abandoned_directory = root.join(".alef-snippet-abandoned");
        std::fs::create_dir_all(abandoned_directory.join("nested")).expect("abandoned scratch");
        std::fs::write(abandoned_directory.join("nested/snippet.go"), "package main\n").expect("abandoned source");
        let abandoned_file = root.join(".tmpabandoned.rb");
        std::fs::write(&abandoned_file, "puts 1\n").expect("abandoned file");

        super::purge_scratch_root_entries(&root, std::time::Duration::ZERO).expect("sweep runs");

        assert!(!abandoned_directory.exists(), "an abandoned directory must be swept");
        assert!(!abandoned_file.exists(), "an abandoned loose file must be swept");
        assert!(root.is_dir(), "the sweep must keep the root itself");
    }

    /// The grace window is load-bearing, not cosmetic: two alef processes can share one working
    /// directory, so a sweep that removed freshly touched entries would delete the other run's
    /// live scratch out from under it. A just-created entry must survive the real entry point. ~keep
    #[test]
    fn the_sweep_spares_scratch_a_concurrent_run_may_still_be_using() {
        let directory = tempfile::tempdir().expect("working directory");
        let root = directory.path().join(SNIPPET_SCRATCH_ROOT);
        let live = root.join(".alef-snippet-live");
        std::fs::create_dir_all(&live).expect("live scratch");

        purge_stale_scratch_root(&root, 0).expect("sweep runs");

        assert!(live.exists(), "a freshly touched entry must survive the sweep");
    }

    #[test]
    fn the_sweep_is_a_no_op_when_the_root_was_never_created() {
        let directory = tempfile::tempdir().expect("working directory");

        purge_stale_scratch_root(&directory.path().join(SNIPPET_SCRATCH_ROOT), 30)
            .expect("sweep tolerates a missing root");
    }
}
