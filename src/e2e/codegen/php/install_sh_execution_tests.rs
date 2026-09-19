//! Executable proof that the generated `install.sh` selects its PIE interpreter by digest,
//! never by what happens to be on `PATH`.
//!
//! The script used to reuse an already-installed `pie` whenever it reported >= 1.3.7. That
//! made the pin and the SHA-256 check apply only on a machine that happened to have no PIE:
//! everywhere else an arbitrary, unverified binary ran instead, and two machines executed two
//! different PIEs. This is the inverse of the injection defects elsewhere in this change --
//! there, hostile input reached a trusted sink; here the sink was trusted and the *binary* was
//! unverified.
//!
//! [`preinstalled_pie_on_path_is_never_executed`] is the marker-file negative control: a fake
//! `pie` sits first on `PATH`, announces a version that would have satisfied the old gate, and
//! drops a marker file if it is ever run. The marker must not exist.
//! [`the_removed_version_sniffing_gate_would_have_executed_the_fake_pie`] runs the *pre-fix*
//! bootstrap through the identical sandbox and requires the marker to appear, so a harness
//! that stopped being able to observe the bypass fails loudly instead of passing vacuously.
//! ~keep

use super::project::{render_install_sh, render_run_tests_php};
use std::path::Path;

/// Marker the fake `pie` drops if it is ever executed.
const PIE_EXECUTED_MARKER: &str = "FAKE_PIE_WAS_EXECUTED";

/// The pre-fix interpreter-selection gate, verbatim. Kept here rather than in the generator so
/// the RED control exercises the exact shell the generator used to emit.
const REMOVED_VERSION_SNIFFING_GATE: &str = r#"
need_pie_install=true
if command -v pie >/dev/null 2>&1; then
  current="$(pie --version 2>&1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo '0.0.0')"
  if printf '%s\n%s\n' "1.3.7" "$current" | sort -V -C; then
    need_pie_install=false
  fi
fi
if [[ "$need_pie_install" == "false" ]]; then
  PIE="pie"
  "$PIE" install "pkg:1.0.0"
  exit 0
fi
"#;

fn write_executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::write(path, body).expect("write script");
    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}

/// Build a `PATH` whose FIRST entry holds a hostile `pie` that reports a version the removed
/// gate would have accepted and drops [`PIE_EXECUTED_MARKER`] into `root` if executed, plus a
/// `curl` that "downloads" a stub instead of reaching the network.
fn poisoned_path(root: &Path) -> String {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).expect("create sandbox bin dir");
    write_executable(
        &bin.join("pie"),
        &format!(
            "#!/bin/sh\ntouch '{}'\necho 'PIE 9.9.9'\nexit 0\n",
            root.join(PIE_EXECUTED_MARKER).display()
        ),
    );
    write_executable(
        &bin.join("curl"),
        concat!(
            "#!/bin/sh\n",
            "out=''\n",
            "while [ $# -gt 0 ]; do\n",
            "  case \"$1\" in --output) out=\"$2\"; shift 2;; *) shift;; esac\n",
            "done\n",
            "printf 'not-the-real-phar' > \"$out\"\n"
        ),
    );
    format!("{}:/usr/bin:/bin", bin.display())
}

/// Run `script` under `bash` with the poisoned `PATH` and `HOME` rooted at `root`.
fn run_bootstrap(root: &Path, script: &str) -> std::process::Output {
    let path = poisoned_path(root);
    let script_path = root.join("bootstrap.sh");
    std::fs::write(&script_path, script).expect("write bootstrap script");
    std::process::Command::new("/bin/bash")
        .arg(&script_path)
        .current_dir(root)
        .env_clear()
        .env("PATH", path)
        .env("HOME", root)
        .output()
        .expect("/bin/bash should start")
}

/// Everything the generated script does before it would invoke `"$PIE"`, plus a line that
/// invokes it -- i.e. the interpreter-selection decision and its consequence, isolated from
/// the `php`/`composer` steps a unit test cannot run.
fn bootstrap_section(content: &str) -> String {
    let start = content.find("PIE_VERSION=").expect("PIE_VERSION pin present");
    let end = content
        .find("# Install the extension binary into the running PHP's extension dir.")
        .expect("bootstrap section ends before the extension install");
    format!(
        "set -euo pipefail\n{}\n\"$PIE\" install \"pkg:1.0.0\"\n",
        &content[start..end]
    )
}

/// NEGATIVE CONTROL. A hostile `pie` first on `PATH`, announcing a version the removed gate
/// accepted, must never be selected. The marker file is the assertion: string-matching the
/// generated script would only prove the text changed, not that the binary cannot be reached.
#[test]
fn preinstalled_pie_on_path_is_never_executed() {
    let root = tempfile::tempdir().expect("tempdir");
    let content = render_install_sh("test/pkg", "ext", "1.0.0");
    let output = run_bootstrap(root.path(), &bootstrap_section(&content));

    assert!(
        !root.path().join(PIE_EXECUTED_MARKER).exists(),
        "the preinstalled `pie` on PATH was executed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "a stub download must not pass the digest check; stdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("checksum mismatch"),
        "the script must reach the digest comparison rather than short-circuiting to the \
         preinstalled binary; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// RED harness proof: the removed gate, run through the identical sandbox, DOES execute the
/// fake `pie`. Without this, the test above could pass because the fake `pie` is unrunnable.
#[test]
fn the_removed_version_sniffing_gate_would_have_executed_the_fake_pie() {
    let root = tempfile::tempdir().expect("tempdir");
    let script = format!("set -euo pipefail\n{REMOVED_VERSION_SNIFFING_GATE}");
    let output = run_bootstrap(root.path(), &script);

    assert!(
        root.path().join(PIE_EXECUTED_MARKER).exists(),
        "the pre-fix gate must execute the fake `pie`, otherwise the control test proves \
         nothing; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The generated script must carry no interpreter-selection branch at all: no `command -v
/// pie`, no version comparison, no `PIE="pie"` fallback to whatever is on `PATH`.
#[test]
fn the_generated_script_has_no_path_based_interpreter_fallback() {
    let content = render_install_sh("test/pkg", "ext", "1.0.0");
    for forbidden in [
        "command -v pie",
        "need_pie_install",
        "PIE=\"pie\"",
        "sort -V -C",
        "1.3.7",
    ] {
        assert!(
            !content.contains(forbidden),
            "install.sh must not reintroduce a PATH-based PIE fallback ({forbidden:?}), got:\n{content}"
        );
    }
    assert!(
        content.contains("PIE=\"$pie_dir/pie\""),
        "the interpreter must always be the verified PHAR, got:\n{content}"
    );
    // The digest gate must precede the only assignment of `$PIE`, so no code path reaches an
    // interpreter that was not compared against the pin.
    let verify = content.find("checksum mismatch").expect("digest gate present");
    let assign = content.find("PIE=\"$pie_dir/pie\"").expect("interpreter assigned");
    assert!(verify < assign, "the digest must be checked before `$PIE` is chosen");
}

/// Extract the single line containing `needle` from `content`, verbatim, so a test can prove
/// it exercises the actual generated line rather than a hand-copied stand-in.
fn extract_line_containing<'a>(content: &'a str, needle: &str) -> &'a str {
    content
        .lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line containing {needle:?} in:\n{content}"))
}

/// Regression for alef issue #368: the canonical registry runner invokes `bash install.sh`
/// and the test runner as two SEPARATE processes, so a bash `export` made by the first is
/// invisible to the second. This runs install.sh's own extension-dir/export lines in one
/// process, then the run_tests.php resolution snippet -- with a clean environment and no
/// local build artifacts anywhere on disk -- in a second, and proves the second still
/// resolves the exact path the first computed, using only PHP's own `extension_dir`.
#[test]
#[allow(clippy::print_stderr)] // narrow: reports a toolchain skip on a developer machine without PHP ~keep
fn install_sh_and_run_tests_php_agree_on_the_pie_path_across_two_processes() {
    let php = match std::process::Command::new("php").arg("--version").output() {
        Ok(output) if output.status.success() => "php",
        _ => {
            eprintln!("skipping: no `php` interpreter on PATH");
            return;
        }
    };

    let root = tempfile::tempdir().expect("tempdir");
    let extension_name = "sample_ext";

    // A fake `extension_dir` PIE would have installed into, with the extension binary
    // actually present -- this is what the running PHP reports via `ini_get`.
    let ext_dir = root.path().join("ext_dir");
    std::fs::create_dir_all(&ext_dir).expect("create fake extension_dir");
    std::fs::write(ext_dir.join(format!("{extension_name}.so")), b"fake extension").expect("write fake .so");

    // A stub `php` on PATH: install.sh only ever calls `php -r '...ini_get("extension_dir")...'`
    // and `php --ini` in the section under test here, so a fixed reply is enough to stand in
    // for the PHP that PIE actually installed into.
    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).expect("create sandbox bin dir");
    write_executable(&bin.join("php"), &format!("#!/bin/sh\necho '{}'\n", ext_dir.display()));
    let stub_path = format!("{}:/usr/bin:/bin", bin.display());

    // Process 1: install.sh's own `EXT_DIR=` and `export PIE_INSTALLED_EXTENSION_PATH=` lines,
    // extracted verbatim from the real generator output, run in their own bash process with
    // `OSTYPE` set to a Darwin value -- the removed `.dylib` branch used to key off exactly
    // this, so a regression there would still show up under this label.
    let install_sh = render_install_sh("test/pkg", extension_name, "1.0.0");
    let extension_name_line = extract_line_containing(&install_sh, "EXTENSION_NAME=");
    let ext_dir_line = extract_line_containing(&install_sh, "EXT_DIR=\"$(php -r");
    let export_line = extract_line_containing(&install_sh, "export PIE_INSTALLED_EXTENSION_PATH=");
    let process_one_script = format!(
        "set -euo pipefail\n{extension_name_line}\n{ext_dir_line}\n{export_line}\necho \"EXPORTED=$PIE_INSTALLED_EXTENSION_PATH\""
    );
    let process_one = std::process::Command::new("/bin/bash")
        .arg("-c")
        .arg(&process_one_script)
        .env_clear()
        .env("PATH", &stub_path)
        .env("HOME", root.path())
        .env("OSTYPE", "darwin23")
        .output()
        .expect("run install.sh's extension-dir/export lines");
    assert!(
        process_one.status.success(),
        "process 1 (install.sh lines) failed, stderr:\n{}",
        String::from_utf8_lossy(&process_one.stderr)
    );
    let process_one_stdout = String::from_utf8_lossy(&process_one.stdout);
    let expected_export = format!("EXPORTED={}/{extension_name}.so", ext_dir.display());
    assert_eq!(
        process_one_stdout.trim(),
        expected_export,
        "install.sh must export the .so path even on a Darwin-labelled run"
    );
    assert!(
        !process_one_stdout.contains(".dylib"),
        "install.sh must never export a .dylib path, got: {process_one_stdout}"
    );

    // Process 2: a FRESH process, env cleared (no PIE_INSTALLED_EXTENSION_PATH -- process 1's
    // export cannot reach it), with no local build artifact on disk anywhere the harness would
    // look. This is exactly what `composer test` sees today: the export from process 1 is gone.
    let run_tests = render_run_tests_php(
        extension_name,
        "sample-ext-php",
        "crates/sample-ext-php",
        "1.0.0",
        crate::e2e::config::DependencyMode::Registry,
    )
    .expect("run_tests.php renders");
    let start = run_tests
        .find("// Check for a PIE-installed extension path")
        .expect("resolution comment present");
    let end = run_tests
        .find("// Neither a local release build")
        .expect("end-of-block marker present");
    let resolution_snippet = &run_tests[start..end];

    let missing_local_build = root.path().join("target/release/nonexistent");
    let script = format!(
        "<?php\n$extPath = '{missing}';\n{resolution_snippet}\necho $extPath;",
        missing = missing_local_build.display()
    );
    let script_path = root.path().join("resolve.php");
    std::fs::write(&script_path, script).expect("write resolution script");

    // `-n` skips the ambient php.ini/conf.d entirely: this machine's own PHP install may
    // declare unrelated real extensions there, which would otherwise fail to load from the
    // overridden `extension_dir` below and spew startup warnings onto stdout, polluting the
    // very output this assertion reads. `-d extension_dir=...` still applies under `-n`.
    let process_two = std::process::Command::new(php)
        .arg("-n")
        .arg("-d")
        .arg(format!("extension_dir={}", ext_dir.display()))
        .arg(&script_path)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .output()
        .expect("run run_tests.php's resolution snippet");
    assert!(
        process_two.status.success(),
        "process 2 (run_tests.php resolution) failed, stderr:\n{}",
        String::from_utf8_lossy(&process_two.stderr)
    );
    let resolved = String::from_utf8_lossy(&process_two.stdout).into_owned();
    let expected_path = format!("{}/{extension_name}.so", ext_dir.display());
    assert_eq!(
        resolved, expected_path,
        "run_tests.php must resolve the same path install.sh installed to, without the env \
         var surviving across the process boundary and without a local build artifact"
    );
    assert_eq!(
        resolved,
        expected_export.trim_start_matches("EXPORTED="),
        "the two independently-run processes must agree on the exact same extension path"
    );
}
