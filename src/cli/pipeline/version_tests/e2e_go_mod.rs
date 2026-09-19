use super::*;

const GO_MOD_E2E: &str = "\
module e2e_go

go 1.26

require (
\tgithub.com/sample_crate-dev/sample_crawler/packages/go v0.3.0-rc.27
\tgithub.com/stretchr/testify v1.11.1
)
";

const GO_MOD_E2E_LOCAL_REPLACE: &str = "\
module e2e_go

go 1.26

require (
\tgithub.com/sample_crate-dev/sample_crawler/packages/go v0.3.0-rc.27
\tgithub.com/stretchr/testify v1.11.1
)

replace github.com/sample_crate-dev/sample_crawler/packages/go => ../../packages/go
";

#[test]
fn sync_e2e_go_mod_updates_library_require_line() {
    let fragment = "github.com/sample_crate-dev/sample_crawler/packages/go";
    let result = sync_e2e_go_mod(GO_MOD_E2E, fragment, "0.3.0-rc.28");
    assert!(result.is_some(), "expected Some when version changes");
    let new = result.unwrap();
    assert!(
        new.contains("github.com/sample_crate-dev/sample_crawler/packages/go v0.3.0-rc.28"),
        "library require line must be updated:\n{new}"
    );
    assert!(
        new.contains("github.com/stretchr/testify v1.11.1"),
        "testify version must be unchanged:\n{new}"
    );
    assert!(!new.contains("v0.3.0-rc.27"), "old version must be gone:\n{new}");
}

#[test]
fn sync_e2e_go_mod_is_idempotent() {
    let fragment = "github.com/sample_crate-dev/sample_crawler/packages/go";
    let first = sync_e2e_go_mod(GO_MOD_E2E, fragment, "0.3.0-rc.28").unwrap();
    let second = sync_e2e_go_mod(&first, fragment, "0.3.0-rc.28");
    assert!(second.is_none(), "second call with same version must be a no-op");
}

#[test]
fn sync_e2e_go_mod_skips_local_replace_placeholder_version() {
    let fragment = "github.com/sample_crate-dev/sample_crawler/packages/go";
    let result = sync_e2e_go_mod(GO_MOD_E2E_LOCAL_REPLACE, fragment, "0.3.0-rc.28");
    assert!(
        result.is_none(),
        "local replace entries keep generated placeholder versions"
    );
}

const GO_MOD_E2E_V2_BLOCK: &str = "\
module e2e_go

go 1.26

require (
\tgithub.com/sample_crate-dev/sample_crawler/packages/go/v2 v2.0.3
\tgithub.com/stretchr/testify v1.11.1
)
";

const GO_MOD_E2E_SINGLE_LINE: &str = "\
module e2e_go

go 1.26

require github.com/sample_crate-dev/sample_crawler/packages/go v0.3.0-rc.27
";

#[test]
fn sync_e2e_go_mod_updates_v2_block_require_line() {
    let module_path = "github.com/sample_crate-dev/sample_crawler/packages/go/v2";
    let result = sync_e2e_go_mod(GO_MOD_E2E_V2_BLOCK, module_path, "2.0.4");
    let new = result.expect("a /v2 module in a require block must be recognized");
    assert!(
        new.contains("github.com/sample_crate-dev/sample_crawler/packages/go/v2 v2.0.4"),
        "v2 library require line must be updated:\n{new}"
    );
    assert!(
        new.contains("github.com/stretchr/testify v1.11.1"),
        "unrelated testify dependency must be unchanged:\n{new}"
    );
    assert!(!new.contains("v2.0.3"), "old version must be gone:\n{new}");
}

#[test]
fn sync_e2e_go_mod_updates_single_line_require() {
    let module_path = "github.com/sample_crate-dev/sample_crawler/packages/go";
    let result = sync_e2e_go_mod(GO_MOD_E2E_SINGLE_LINE, module_path, "0.3.0-rc.28");
    let new = result.expect("a single-line require statement must be recognized");
    assert!(
        new.contains("require github.com/sample_crate-dev/sample_crawler/packages/go v0.3.0-rc.28"),
        "single-line require must be updated:\n{new}"
    );
    assert!(!new.contains("v0.3.0-rc.27"), "old version must be gone:\n{new}");
}

#[test]
fn sync_e2e_go_mod_leaves_unrelated_dependency_untouched() {
    let module_path = "github.com/sample_crate-dev/sample_crawler/packages/go";
    let result = sync_e2e_go_mod(GO_MOD_E2E, module_path, "0.3.0-rc.28").unwrap();
    assert!(
        result.contains("github.com/stretchr/testify v1.11.1"),
        "unrelated dependency must not be touched:\n{result}"
    );
}
