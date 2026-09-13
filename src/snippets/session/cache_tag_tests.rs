//! Session preparation is what creates a per-project `.alef/` in a consumer's tree, and these pin
//! that it leaves it *provable*.
//!
//! External catalogues (voom, `tar --exclude-caching`, `rsync --exclude-tag`, Borg, restic) probe
//! for a `CACHEDIR.TAG` *directly inside* the directory they are looking at and deliberately do
//! not accept one found higher up, because that would let a single tag license every directory
//! beneath it. Every session site used to tag only the leaf it was about to write into -- two to
//! four levels below `.alef/` -- so the directory those tools actually meet carried nothing.
//! Measured across one machine: 82 of 83 `.alef/` directories had no tag at their own root,
//! holding 18.86 GB of regenerable cache no such tool would reclaim. ~keep

use super::*;
use std::collections::BTreeMap;

/// A session's workspace sits four levels below `.alef/`, and nothing but `.alef/` itself answers
/// for `.alef/`.
#[test]
fn a_session_workspace_tags_the_alef_root_as_well_as_the_fingerprint_directory() {
    let directory = tempfile::tempdir().expect("temp directory");
    let session = ValidationSession {
        language: Language::Python,
        working_directory: directory.path().to_path_buf(),
        manifest: None,
        fingerprint: "cache-tag-fixture".into(),
        env: BTreeMap::new(),
        include_paths: Vec::new(),
        rust_features: Vec::new(),
        rust_dependencies: BTreeMap::new(),
    };

    let workspace = session.workspace_directory().expect("workspace directory");

    for tagged in [directory.path().join(".alef"), workspace] {
        assert!(
            crate::core::cache_dir::is_tagged(&tagged),
            "{} must carry a valid CACHEDIR.TAG",
            tagged.display()
        );
    }
}

/// The toolchain caches are the big ones -- a cargo target directory per session key -- and
/// preparation creates them whether or not a `before` hook runs, so this goes through the real
/// `prepare_sessions_isolated` entry point rather than calling the tagging helper directly. ~keep
#[test]
fn preparing_a_session_tags_the_alef_root_as_well_as_its_toolchain_caches() {
    let directory = tempfile::tempdir().expect("temp directory");
    let mut specs = HashMap::new();
    specs.insert(
        "python".into(),
        SessionSpec {
            language: Language::Python,
            working_directory: directory.path().to_path_buf(),
            manifest: None,
            before: Vec::new(),
            env: BTreeMap::new(),
            include_paths: Vec::new(),
            rust_features: Vec::new(),
            rust_dependencies: BTreeMap::new(),
        },
    );

    let prepared = prepare_sessions_isolated(&specs, 5);

    let session = prepared.sessions.get("python").expect("prepared python session");
    for cache in session.cache_directories().directories() {
        assert!(
            crate::core::cache_dir::is_tagged(cache),
            "{} must carry a valid CACHEDIR.TAG",
            cache.display()
        );
    }
    let root = directory.path().join(".alef");
    assert!(
        crate::core::cache_dir::is_tagged(&root),
        "{} must carry a valid CACHEDIR.TAG",
        root.display()
    );
}
