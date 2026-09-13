use alef::backends::pyo3::Pyo3Backend;
use alef::cli::pipeline;
use alef::core::backend::Backend;
use alef::core::config::NewAlefConfig;
use alef::core::ir::{HandlerShape, RegistrationVariantStyle};

const SERVICE_SOURCE: &str = r#"
pub struct App;

impl App {
    pub fn new() -> Self { Self }

    pub fn route(self, method: Method, path: String, handler: HandlerImpl) -> Self {
        self
    }

    pub async fn run(self) -> Result<(), String> { Ok(()) }
}

pub enum Method { Get }
pub struct HandlerImpl;
pub trait Handler { async fn call(&self, request: Request) -> Response; }
pub struct Request;
pub struct Response;
"#;

const CONFIG_TEMPLATE: &str = r#"
[workspace]
languages = ["python"]

[[crates]]
name = "neutral-service"
sources = ["__SOURCE__"]
version_from = "__VERSION__"

[[crates.services]]
owner_type = "App"
constructor = "new"

[[crates.services.registrations]]
method = "route"
callback_param = "handler"
callback_bound = "IntoHandler"
callback_contract = "Handler"
handler_shape = "bare_callable"

[[crates.services.registrations.variants]]
name = "get"
fixed = { method = "Get" }
style = "verb_decorator"
languages = { python = { style = "decorator", handler_shape = "introspect_params", method_prefix = "Map" } }

[[crates.services.entrypoints]]
method = "run"
kind = "run"

[[crates.handler_contracts]]
trait_name = "Handler"
dispatch_method = "call"
wire_request_type = "Request"
wire_response_type = "Response"
"#;

/// Render a filesystem path for interpolation into a TOML *basic* (double-quoted) string.
///
/// A Windows temp path is `C:\\Users\\runneradmin\\AppData\\...`, and `\\U` / `\\A` are not valid
/// TOML escape sequences -- interpolating one verbatim makes the fixture config unparseable, so
/// the test panicked on Windows only, at its own `toml::from_str`, before reaching what it exists
/// to check. ~keep
fn toml_basic_string_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

fn write_fixture() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let directory = tempfile::tempdir().expect("create fixture directory");
    let source_path = directory.path().join("lib.rs");
    let version_path = directory.path().join("Cargo.toml");
    let config_path = directory.path().join("alef.toml");
    std::fs::write(&source_path, SERVICE_SOURCE).expect("write service source");
    std::fs::write(
        &version_path,
        "[package]\nname = \"neutral-service\"\nversion = \"0.1.0\"\n",
    )
    .expect("write version manifest");
    let config = CONFIG_TEMPLATE
        .replace("__SOURCE__", &toml_basic_string_path(&source_path))
        .replace("__VERSION__", &toml_basic_string_path(&version_path));
    std::fs::write(&config_path, &config).expect("write Alef config");
    (directory, config_path, config)
}

#[test]
fn service_variant_language_override_survives_parse_extraction_and_generation() {
    let (_directory, config_path, config_text) = write_fixture();
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../schemas/alef.schema.json")).expect("parse committed config schema");
    let validator = jsonschema::validator_for(&schema).expect("compile committed config schema");
    let config_value: toml::Value = toml::from_str(&config_text).expect("parse config as TOML value");
    let json_value = serde_json::to_value(config_value).expect("convert config to JSON value");
    assert!(
        validator.is_valid(&json_value),
        "committed schema must accept languages map"
    );

    let raw: NewAlefConfig = toml::from_str(&config_text).expect("parse service variant languages map");
    let resolved = raw.resolve().expect("resolve fixture config").remove(0);

    let api = pipeline::extract(&resolved, &config_path, true).expect("extract configured service");
    let variant = &api.services[0].registrations[0].variants[0];
    let effective = variant.resolved_for("python", HandlerShape::BareCallable);
    assert_eq!(effective.style, RegistrationVariantStyle::Decorator);
    assert_eq!(effective.handler_shape, HandlerShape::IntrospectParams);
    assert_eq!(variant.method_name_for("python"), "Mapget");

    let files = Pyo3Backend
        .generate_service_api(&api, &resolved)
        .expect("generate Python service API");
    let service = files
        .iter()
        .find(|file| file.path.ends_with("service.py"))
        .expect("generated Python service module");
    assert!(service.content.contains("handler: Callable[..., Any] | None = None"));
    assert!(!service.content.contains("def get_decorator("));
}

/// Pins [`toml_basic_string_path`]'s escaping against a Windows-shaped path, so the fix is proven
/// on every platform rather than only where the bug reproduces: the unescaped form is not valid
/// TOML at all, and the escaped form must round-trip back to the original path.
#[test]
fn a_windows_path_interpolates_into_a_parseable_toml_basic_string() {
    let windows_path = std::path::Path::new(r"C:\Users\runneradmin\AppData\Local\Temp\x\lib.rs");

    let unescaped = format!("source = \"{}\"\n", windows_path.to_string_lossy());
    assert!(
        toml::from_str::<toml::Value>(&unescaped).is_err(),
        "the unescaped form must be the failure this helper exists to prevent"
    );

    let escaped = format!("source = \"{}\"\n", toml_basic_string_path(windows_path));
    let parsed: toml::Value = toml::from_str(&escaped).expect("escaped path must parse as TOML");
    assert_eq!(
        parsed["source"].as_str().expect("source is a string"),
        windows_path.to_string_lossy()
    );
}
