//! Cache Directory Tagging (<https://bford.info/cachedir/>) for every directory alef uses purely
//! as regenerable cache or scratch space.
//!
//! A directory tagged this way is a stable, tool-agnostic declaration that its contents can be
//! deleted and rebuilt for free. Backup and sync tools that honour the spec (`tar
//! --exclude-caching`, `rsync --exclude-tag`, Borg, restic, several `du` variants, ...) skip it
//! automatically, the same way cargo's `target/` and `~/.cargo/registry`, uv's cache, and
//! Gradle's `~/.gradle/caches` already do. It also lets external tooling recognise an alef cache
//! directory without depending on alef's private file names inside it (`zig-hashes.json`,
//! `ir.json`, ...), which are free to change in any refactor.
//!
//! [`ensure_cache_dir`] is the single call every cache-directory creation site in this crate goes
//! through, so the tag is written -- or correctly left alone -- in exactly one place. Do not
//! duplicate this logic at a call site; add a call to [`ensure_cache_dir`] instead. It must never
//! be pointed at a directory that holds committed, hand-editable, or otherwise non-regenerable
//! content -- see the callers in `cli::cache` that create `base_dir` itself for the committed
//! `.alef-ownership.toml` / `.alef-toml-merge-provenance.toml` records, which deliberately do
//! *not* call this function. ~keep

use std::io;
use std::path::Path;

/// The exact CACHEDIR.TAG signature, byte-for-byte per the spec at <https://bford.info/cachedir/>.
/// A conforming reader requires this to be the tag file's first line and nothing else on that line
/// -- not a prefix, not a substring elsewhere in the file.
///
/// This is a single fixed string shared by every tool and every directory in the world; it encodes
/// nothing about alef and must never be customised. Deriving a per-tool variant of it -- which this
/// constant previously did -- produces a file that every real consumer (`tar --exclude-caching`,
/// `rsync --exclude-tag`, Borg, restic) silently ignores, so the directory is never skipped and the
/// whole point of writing the tag is lost with no error anywhere. ~keep
const SIGNATURE_LINE: &str = "Signature: 8a477f597d28d172789f06886806bc55";

/// The malformed signature alef itself wrote before the spec value above was corrected.
///
/// This exists solely so [`ensure_tag`] can recognise its OWN past output and repair it in place.
/// It is deliberately not accepted by [`has_valid_signature`]: a file carrying it is invalid, and
/// treating it as valid would re-entrench the bug. The distinction that makes rewriting safe here
/// is authorship, not validity -- `ensure_tag` refuses to overwrite a tag file it cannot prove it
/// wrote, and this string is proof it did. ~keep
const LEGACY_ALEF_SIGNATURE_LINE: &str = "Signature: 8985a1d0364e3d1e-cache-directory-tag";

const TAG_FILE_NAME: &str = "CACHEDIR.TAG";

/// Body written for a freshly created tag. Everything after the signature line is free-form
/// commentary a backup tool ignores; naming alef and linking the spec here is convention, not
/// contract -- see [`SIGNATURE_LINE`] for the one line that is.
fn tag_body() -> String {
    format!(
        "{SIGNATURE_LINE}\n\
         # This directory contains a cache created by alef. Deleting it only costs a rebuild.\n\
         # For information about cache directory tags see https://bford.info/cachedir/\n"
    )
}

/// Create `dir` (and any missing parents) and make sure it carries a valid `CACHEDIR.TAG`.
///
/// Directory creation failures are returned to the caller -- every existing call site already
/// treats "the cache directory could not be created" as fatal to whatever write was about to
/// follow, and that behaviour is unchanged here. Tag failures are never returned: per this
/// repo's tracing level contract a cache directory that works but is untagged is a
/// degraded-but-continuing condition (`WARN`), not a build failure, so a permissions error or an
/// unrecognised file at the tag path is logged and swallowed rather than propagated. See
/// [`ensure_tag`] for the idempotence and invalid-tag rules.
pub fn ensure_cache_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    ensure_tag(dir);
    Ok(())
}

/// Create `root` and every cache directory beneath it named by `descendant`, tagging `root`
/// itself as well as the leaf.
///
/// External tooling identifies an alef cache by finding a `CACHEDIR.TAG` *directly inside* the
/// directory it is looking at -- a reader that stops at the first tag at or above a candidate
/// would let one tag high in a tree license everything beneath it, so conforming catalogues probe
/// the exact directory instead. Tagging only the leaf therefore leaves `.alef/` itself unproven on
/// any run that happens to create a child first, which is silent and looks identical to a tree
/// that was never alef's. ~keep
pub fn ensure_cache_dir_under(root: &Path, descendant: &Path) -> io::Result<()> {
    ensure_cache_dir(root)?;
    ensure_cache_dir(descendant)
}

/// The one directory name alef claims inside a consumer's tree. Everything alef caches in-tree
/// lives under it, `**/.alef/` is ignored wherever alef generates, and it is therefore the
/// directory an external catalogue actually meets and probes.
pub(crate) const PROJECT_CACHE_DIR_NAME: &str = ".alef";

/// Create and tag `dir`, and tag the enclosing [`PROJECT_CACHE_DIR_NAME`] root as well when `dir`
/// sits inside one.
///
/// This is [`ensure_cache_dir_under`] for the per-project sites, which know their leaf but not the
/// `.alef/` root it descends from: the snippet verdict cache's directory is
/// `docs.snippets.cache_dir` straight from a consumer's config, and the session workspace,
/// toolchain cache, and scratch roots are each nested two to four levels below `.alef/` behind a
/// path constant of their own. Every one of them used to tag only its leaf, which leaves `.alef/`
/// itself unproven -- see [`ensure_cache_dir_under`] for why a tag deeper in the tree does not
/// answer for it.
///
/// The root is found by name rather than by climbing a fixed number of parents, and a `dir` with
/// no `.alef/` above it is tagged alone exactly as before. That is the safe reading of a
/// *configured* cache directory: `cache_dir = "build/snippet-cache"` must never get `build/`
/// tagged on the strength of being somebody's parent, because a tag there tells every backup tool
/// on the machine to skip a directory alef does not own. Only a component alef named is ever
/// tagged. The outermost match wins, so the tag lands on the directory an external reader meets
/// first and stops recursing at. ~keep
pub(crate) fn ensure_project_cache_dir(dir: &Path) -> io::Result<()> {
    match project_cache_root(dir) {
        Some(root) => ensure_cache_dir_under(root, dir),
        None => ensure_cache_dir(dir),
    }
}

/// The outermost ancestor of `dir` -- `dir` itself included -- named [`PROJECT_CACHE_DIR_NAME`].
fn project_cache_root(dir: &Path) -> Option<&Path> {
    dir.ancestors()
        .filter(|ancestor| ancestor.file_name() == Some(std::ffi::OsStr::new(PROJECT_CACHE_DIR_NAME)))
        .last()
}

/// Whether `dir` carries a tag a conforming reader would accept right now.
///
/// Test-only, and crate-visible rather than module-private so a test anywhere in the crate can
/// assert "this directory is recognisable as a cache" without restating [`SIGNATURE_LINE`] --
/// restating it is exactly the duplication this module's doc forbids, and a copy that drifts
/// would assert the tag is present while every real reader disagrees. ~keep
#[cfg(test)]
pub(crate) fn is_tagged(dir: &Path) -> bool {
    std::fs::read(dir.join(TAG_FILE_NAME)).is_ok_and(|content| has_valid_signature(&content))
}

/// Idempotently ensure `dir` -- which must already exist -- carries a valid `CACHEDIR.TAG`,
/// without creating `dir` itself. Split out purely so [`ensure_cache_dir`]'s own doc can stay
/// focused on the create-then-tag contract every caller actually depends on.
fn ensure_tag(dir: &Path) {
    let tag_path = dir.join(TAG_FILE_NAME);
    match std::fs::read(&tag_path) {
        Ok(existing) => {
            // Idempotence: a valid tag is never rewritten, so an operator's edited comment body
            // and the file's mtime both survive every subsequent run.
            if has_valid_signature(&existing) {
                return;
            }
            // Repair alef's own past output. A file whose first line is the malformed signature
            // alef used to write is, by that signature, a file alef wrote -- so rewriting it does
            // not violate the "never clobber content we did not write" rule below; authorship is
            // exactly what that rule protects, and this proves it. Without this branch every
            // already-tagged cache on disk would stay unrecognised by backup tools forever, since
            // the corrected signature makes all of them fail `has_valid_signature` and fall
            // through to the warn-and-leave path. ~keep
            if first_line(&existing) == LEGACY_ALEF_SIGNATURE_LINE.as_bytes() {
                if let Err(write_error) = std::fs::write(&tag_path, tag_body()) {
                    tracing::warn!(
                        path = %tag_path.display(),
                        error = %write_error,
                        "could not rewrite a {TAG_FILE_NAME} carrying alef's superseded signature; \
                         backup tools that honour the tag will keep descending into this cache"
                    );
                }
                return;
            }
            // Invalid tag: something occupies this reserved name whose first line is not the
            // signature -- foreign content, or a truncated write from a crash. Never overwrite
            // content this function did not itself write; the safe failure here is "this run's
            // cache directory stays untagged," not "silently destroy a file we don't recognise."
            tracing::warn!(
                path = %tag_path.display(),
                "a file named {TAG_FILE_NAME} exists here but its first line is not the \
                 cache-directory-tag signature; leaving it untouched instead of overwriting \
                 content alef did not write -- this directory will not be recognised as a cache \
                 by tools that honour the tag until it is repaired or removed by hand"
            );
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Err(write_error) = std::fs::write(&tag_path, tag_body()) {
                tracing::warn!(
                    path = %tag_path.display(),
                    error = %write_error,
                    "could not write {TAG_FILE_NAME}; the cache directory itself still works, \
                     but backup/sync tools that honour the tag will not skip it"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                path = %tag_path.display(),
                error = %error,
                "could not read the existing {TAG_FILE_NAME} to check it; leaving this cache \
                 directory untagged for this run rather than guessing whether it is safe to write"
            );
        }
    }
}

/// Whether `content`'s first line -- up to but not including the first `\n` -- is byte-identical
/// to [`SIGNATURE_LINE`]. Deliberately not a substring/`contains` check: a signature on line two
/// is not a valid tag per spec, and a reader that accepted it would tag directories a real
/// CACHEDIR.TAG-aware tool does not.
fn has_valid_signature(content: &[u8]) -> bool {
    first_line(content) == SIGNATURE_LINE.as_bytes()
}

/// `content` up to but not including the first `\n`, or all of it when there is no newline.
///
/// Extracted so the validity check and [`ensure_tag`]'s legacy-signature repair read the first
/// line the same way. They ask different questions of it -- "is this the spec signature" versus
/// "is this alef's own superseded signature" -- and two hand-inlined splits that drifted apart
/// would make a file simultaneously invalid and unrepairable. ~keep
fn first_line(content: &[u8]) -> &[u8] {
    content.split(|&byte| byte == b'\n').next().unwrap_or(content)
}

#[cfg(test)]
mod tests {

    /// External catalogues identify an alef cache by probing for a `CACHEDIR.TAG` *directly
    /// inside* the directory they are looking at -- they deliberately do not accept a tag found
    /// higher up, because that would let one tag license every directory beneath it. So a nested
    /// cache directory being tagged says nothing about its root, and a root left untagged is
    /// invisible in exactly the silent, safe-looking direction. ~keep
    #[test]
    fn ensure_cache_dir_under_tags_the_root_as_well_as_the_leaf() {
        let base = tempfile::tempdir().expect("tempdir");
        let root = base.path().join(".alef");
        let leaf = root.join("sample_crate").join("hashes");

        ensure_cache_dir_under(&root, &leaf).expect("root and leaf are created");

        for dir in [&root, &leaf] {
            let tag = dir.join(TAG_FILE_NAME);
            let first_line = std::fs::read_to_string(&tag)
                .unwrap_or_else(|error| panic!("no tag at {}: {error}", dir.display()))
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                first_line,
                SIGNATURE_LINE,
                "the tag at {} must carry the exact signature",
                dir.display()
            );
        }
    }

    /// The per-project sites reach their `.alef/` root through the name, not through a fixed
    /// number of parents: the leaf sits two levels below it for the snippet verdict cache and four
    /// for a session workspace, and no caller passes the root in. ~keep
    #[test]
    fn ensure_project_cache_dir_tags_the_enclosing_alef_root_however_deep_the_leaf_is() {
        let base = tempfile::tempdir().expect("tempdir");
        let root = base.path().join(".alef");
        let leaf = root.join("snippets").join("sessions").join("fingerprint");

        ensure_project_cache_dir(&leaf).expect("root and leaf are created");

        for dir in [&root, &leaf] {
            assert!(is_tagged(dir), "{} must carry a valid tag", dir.display());
        }
    }

    /// The safety half of finding the root by name. `docs.snippets.cache_dir` is a consumer's own
    /// configuration and need not live under `.alef/` at all, so a helper that tagged whatever
    /// happened to be the parent would write "skip this, it is regenerable" into a directory alef
    /// does not own -- and every backup tool on the machine would believe it. ~keep
    #[test]
    fn ensure_project_cache_dir_never_tags_a_parent_outside_an_alef_root() {
        let base = tempfile::tempdir().expect("tempdir");
        let configured = base.path().join("build").join("snippet-cache");

        ensure_project_cache_dir(&configured).expect("configured cache directory is created");

        assert!(
            is_tagged(&configured),
            "the configured cache directory itself must be tagged"
        );
        assert!(
            !is_tagged(&base.path().join("build")),
            "a parent alef did not name must never be tagged"
        );
    }

    /// Control: tagging only a leaf must leave its root untagged, which is the state the helper
    /// above exists to prevent. Without this the test above would pass just as well if
    /// `ensure_cache_dir` had always tagged ancestors. ~keep
    #[test]
    fn ensure_cache_dir_alone_does_not_tag_the_parent() {
        let base = tempfile::tempdir().expect("tempdir");
        let root = base.path().join(".alef");
        let leaf = root.join("sample_crate");

        ensure_cache_dir(&leaf).expect("leaf is created");

        assert!(leaf.join(TAG_FILE_NAME).is_file(), "the leaf itself must be tagged");
        assert!(
            !root.join(TAG_FILE_NAME).exists(),
            "ensure_cache_dir must tag only the directory it is given"
        );
    }
    use super::*;

    #[test]
    fn creating_a_cache_dir_writes_a_tag_with_the_exact_signature_as_its_first_line() {
        let root = tempfile::tempdir().expect("temp dir");
        let cache_dir = root.path().join("cache");

        ensure_cache_dir(&cache_dir).expect("ensure_cache_dir must succeed");

        assert!(cache_dir.is_dir(), "the cache directory itself must have been created");
        let tag_content = std::fs::read_to_string(cache_dir.join(TAG_FILE_NAME)).expect("read CACHEDIR.TAG");
        let first_line = tag_content.lines().next().expect("tag file must have a first line");
        assert_eq!(
            first_line, SIGNATURE_LINE,
            "the first line must equal the signature exactly, not merely contain it"
        );
    }

    #[test]
    fn a_second_run_does_not_rewrite_an_existing_valid_tag() {
        let root = tempfile::tempdir().expect("temp dir");
        let cache_dir = root.path().join("cache");
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
        let tag_path = cache_dir.join(TAG_FILE_NAME);
        let custom_body = format!("{SIGNATURE_LINE}\n# a human-edited comment that must survive\n");
        std::fs::write(&tag_path, &custom_body).expect("plant an existing valid tag");
        let mtime_before = std::fs::metadata(&tag_path)
            .expect("stat before")
            .modified()
            .expect("mtime");

        // Guarantee the filesystem mtime clock has a chance to move, so an accidental rewrite
        // would be observable even on filesystems with coarse mtime resolution.
        std::thread::sleep(std::time::Duration::from_millis(1100));

        ensure_cache_dir(&cache_dir).expect("second call must succeed");

        let content_after = std::fs::read_to_string(&tag_path).expect("read tag after second run");
        let mtime_after = std::fs::metadata(&tag_path)
            .expect("stat after")
            .modified()
            .expect("mtime");
        assert_eq!(
            content_after, custom_body,
            "an existing valid tag's content, including a user-added comment, must survive byte-for-byte"
        );
        assert_eq!(
            mtime_before, mtime_after,
            "an existing valid tag must not be rewritten -- its mtime must not move"
        );
    }

    #[test]
    fn an_invalid_existing_tag_file_is_left_untouched_rather_than_clobbered() {
        let root = tempfile::tempdir().expect("temp dir");
        let cache_dir = root.path().join("cache");
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
        let tag_path = cache_dir.join(TAG_FILE_NAME);
        let foreign_content = "not a cache directory tag\njust some other file\n";
        std::fs::write(&tag_path, foreign_content).expect("plant a foreign, non-signature file");

        ensure_cache_dir(&cache_dir).expect("must not error even when the tag path is occupied");

        let content_after = std::fs::read_to_string(&tag_path).expect("read after ensure_cache_dir");
        assert_eq!(
            content_after, foreign_content,
            "a file at CACHEDIR.TAG whose first line is not the signature must never be overwritten"
        );
    }

    #[test]
    fn a_directory_that_cannot_be_tagged_is_still_created_and_usable() {
        let root = tempfile::tempdir().expect("temp dir");
        let cache_dir = root.path().join("cache");
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
        // Occupy the tag's reserved name with a directory instead of a file, so both the read
        // (to check for an existing valid tag) and any write attempt fail without relying on
        // platform-specific permission bits.
        std::fs::create_dir_all(cache_dir.join(TAG_FILE_NAME)).expect("occupy tag path with a directory");

        let result = ensure_cache_dir(&cache_dir);

        assert!(
            result.is_ok(),
            "a cache directory that cannot be tagged must still succeed, not fail the caller: {result:?}"
        );
        assert!(
            cache_dir.is_dir(),
            "the cache directory itself must remain usable regardless of the tag outcome"
        );
    }

    #[test]
    fn has_valid_signature_rejects_the_signature_on_a_later_line() {
        // Proves this cannot be satisfied by a `contains`-style check: the signature is present
        // in the bytes, but not as the first line, so it must be rejected.
        let content = format!("not the first line\n{SIGNATURE_LINE}\n");
        assert!(!has_valid_signature(content.as_bytes()));
    }

    /// The one test in this module that can detect a WRONG `SIGNATURE_LINE`.
    ///
    /// Every other test here compares against `SIGNATURE_LINE`, so they all pass for any value of
    /// it -- a self-referential constant test cannot detect a wrong constant, and for a long time
    /// this module's full suite passed while alef wrote a signature no CACHEDIR.TAG consumer
    /// recognised. The spec bytes are therefore duplicated literally here, NOT referenced from the
    /// constant: that duplication is the entire point, and anyone "tidying" it by substituting
    /// `SIGNATURE_LINE` would silently delete the only real check. The value is fixed forever by
    /// <https://bford.info/cachedir/> and is identical for every tool, so it cannot legitimately
    /// drift. ~keep
    #[test]
    fn signature_line_is_the_literal_bytes_the_cachedir_spec_mandates() {
        let spec_bytes: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";
        assert_eq!(
            SIGNATURE_LINE.as_bytes(),
            spec_bytes,
            "SIGNATURE_LINE must be the spec's single fixed signature; a per-tool variant is \
             ignored by every consumer that honours the tag"
        );
        assert_eq!(spec_bytes.len(), 43, "the spec signature is exactly 43 bytes");
    }

    /// A tag file carrying alef's own superseded signature is rewritten in place, so caches
    /// already on disk become recognisable instead of staying invalid forever behind the
    /// warn-and-leave path.
    #[test]
    fn a_tag_carrying_alefs_superseded_signature_is_repaired_in_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tag_path = dir.path().join(TAG_FILE_NAME);
        std::fs::write(&tag_path, format!("{LEGACY_ALEF_SIGNATURE_LINE}\n# stale body\n")).expect("seed");

        ensure_tag(dir.path());

        let repaired = std::fs::read(&tag_path).expect("read back");
        assert!(
            has_valid_signature(&repaired),
            "the superseded signature must be rewritten to the spec one, got: {}",
            String::from_utf8_lossy(&repaired)
        );
    }

    /// The repair above must not become a licence to overwrite anything invalid: a tag path
    /// holding content alef did not write is still left alone.
    #[test]
    fn the_legacy_repair_does_not_clobber_foreign_content_at_the_tag_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tag_path = dir.path().join(TAG_FILE_NAME);
        std::fs::write(&tag_path, b"something else entirely\n").expect("seed");

        ensure_tag(dir.path());

        assert_eq!(
            std::fs::read(&tag_path).expect("read back"),
            b"something else entirely\n",
            "foreign content at the tag path must survive untouched"
        );
    }

    #[test]
    fn has_valid_signature_accepts_exactly_the_signature_with_trailing_content() {
        let content = format!("{SIGNATURE_LINE}\n# trailing comment\n");
        assert!(has_valid_signature(content.as_bytes()));
    }
}
