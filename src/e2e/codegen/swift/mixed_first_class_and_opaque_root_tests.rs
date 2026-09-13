//! Regression coverage for the exact shape a peer report's control relied on: a module where one
//! call's result type is first-class (Codable struct, stored properties) and a DIFFERENT call's
//! result type is opaque (typealias to the swift-bridge class, method-call accessors) *at the
//! same time*, with neither call configuring an explicit cross-language `result_type` override.
//!
//! This settles two things the peer report's own control could not, because it only exercised
//! one type:
//!
//! 1. The e2e resolver does not simply default every path to opaque syntax — it tracks a real
//!    per-type set, matching `FirstClassResult` against the SAME set the binding emitter's own
//!    `first_class_field_supported` fixed point would produce for the same IR. This is proven by
//!    `promoted_result_type_root_tests.rs`'s control test on the SAME classifier the mixed test
//!    here calls into.
//! 2. Before the `test_method.rs` fix (falling `fixture_root_type`/`exclusion_root_type` back to
//!    `call_root_type`, the same IR-derived answer `ir_enum_map`/`ir_collection_map`/
//!    `ir_result_field_map` already use), the OPAQUE call rendered correctly by accident — an
//!    unresolved Swift root and a resolved-but-opaque root both default to method-call syntax —
//!    while the FIRST-CLASS call rendered wrong, because `swift_call_result_type` never learns a
//!    call's result type without an explicit override. Both assertions in this file must hold
//!    simultaneously post-fix; only the first-class one was failing pre-fix.
use crate::core::config::ResolvedCrateConfig;
use crate::core::ir::{FieldDef, FunctionDef, TypeDef, TypeRef};
use crate::e2e::config::{CallConfig, E2eConfig};
use crate::e2e::fixture::{Assertion, Fixture};

fn mixed_ir() -> (Vec<TypeDef>, Vec<FunctionDef>) {
    let type_defs = vec![
        TypeDef {
            name: "FirstClassResult".to_string(),
            has_serde: true,
            fields: vec![FieldDef {
                name: "label".to_string(),
                ty: TypeRef::String,
                ..FieldDef::default()
            }],
            ..TypeDef::default()
        },
        TypeDef {
            name: "OpaqueResult".to_string(),
            has_serde: true,
            fields: vec![FieldDef {
                name: "payload".to_string(),
                // `Bytes` is rejected by `first_class_field_supported` unconditionally, so this
                // type can never be promoted -- the reliable "stays opaque" control.
                ty: TypeRef::Bytes,
                ..FieldDef::default()
            }],
            ..TypeDef::default()
        },
    ];
    let functions = vec![
        FunctionDef {
            name: "make_first_class".to_string(),
            return_type: TypeRef::Named("FirstClassResult".to_string()),
            ..FunctionDef::default()
        },
        FunctionDef {
            name: "make_opaque".to_string(),
            return_type: TypeRef::Named("OpaqueResult".to_string()),
            ..FunctionDef::default()
        },
    ];
    (type_defs, functions)
}

fn render_for(
    call_name: &str,
    field: &str,
    type_defs: &[TypeDef],
    functions: &[FunctionDef],
    e2e_config: &E2eConfig,
    swift_first_class_map: &crate::e2e::field_access::SwiftFirstClassMap,
) -> String {
    let fixture = Fixture {
        id: format!("{call_name}_smoke"),
        description: "mixed first-class/opaque smoke".to_string(),
        call: Some(call_name.to_string()),
        assertions: vec![Assertion {
            assertion_type: "equals".to_string(),
            field: Some(field.to_string()),
            value: Some(serde_json::Value::String("x".to_string())),
            ..Assertion::default()
        }],
        ..Fixture::default()
    };
    let config = ResolvedCrateConfig {
        name: "sample".into(),
        ..ResolvedCrateConfig::default()
    };
    let mut out = String::new();
    super::test_method::render_test_method(
        &mut out,
        &fixture,
        e2e_config,
        call_name,
        "result",
        &[],
        false,
        None,
        swift_first_class_map,
        "Sample",
        &config,
        type_defs,
        &[],
        functions,
        &[],
    );
    out
}

/// Neither call configures a `result_type` override for any language -- the common case, and
/// the one `swift_call_result_type` alone cannot resolve.
fn e2e_config_no_overrides() -> E2eConfig {
    let mut e2e_config = E2eConfig::default();
    e2e_config.calls.insert(
        "make_first_class".to_string(),
        CallConfig {
            function: "make_first_class".to_string(),
            result_var: "result".to_string(),
            ..CallConfig::default()
        },
    );
    e2e_config.calls.insert(
        "make_opaque".to_string(),
        CallConfig {
            function: "make_opaque".to_string(),
            result_var: "result".to_string(),
            ..CallConfig::default()
        },
    );
    e2e_config
}

/// The binding-emitter-mirroring classifier must promote `FirstClassResult` and must NOT
/// promote `OpaqueResult`, for the identical IR both calls share -- proves the per-type
/// classification genuinely tracks a set rather than defaulting uniformly.
#[test]
fn the_classifier_disagrees_about_the_two_types_in_the_same_module() {
    let (type_defs, _functions) = mixed_ir();
    let e2e_config = e2e_config_no_overrides();
    let call = e2e_config.calls["make_first_class"].clone();
    let map = super::values::build_swift_first_class_map(&type_defs, &[], &e2e_config, &call);
    assert!(
        map.is_first_class(Some("FirstClassResult")),
        "FirstClassResult must be classified first-class"
    );
    assert!(
        !map.is_first_class(Some("OpaqueResult")),
        "OpaqueResult (a Bytes field) must stay opaque"
    );
}

/// A call whose result IS the promoted type, with no override configured, must render property
/// syntax for its leaf field.
#[test]
fn the_first_class_call_renders_property_syntax_with_no_override_configured() {
    let (type_defs, functions) = mixed_ir();
    let e2e_config = e2e_config_no_overrides();
    let call = e2e_config.calls["make_first_class"].clone();
    let map = super::values::build_swift_first_class_map(&type_defs, &[], &e2e_config, &call);
    let out = render_for("make_first_class", "label", &type_defs, &functions, &e2e_config, &map);
    assert!(
        out.contains("XCTAssertEqual(result.label, \"x\")"),
        "first-class result's leaf must render property syntax, got:\n{out}"
    );
    assert!(
        !out.contains("result.label()"),
        "must not render method-call syntax, got:\n{out}"
    );
}

/// A call whose result is the opaque type, with no override configured, must keep method-call
/// syntax for its leaf field -- proving the opaque side was never at risk and isolating the fix
/// to the first-class side.
#[test]
fn the_opaque_call_renders_method_call_syntax_with_no_override_configured() {
    let (type_defs, functions) = mixed_ir();
    let e2e_config = e2e_config_no_overrides();
    let call = e2e_config.calls["make_first_class"].clone();
    let map = super::values::build_swift_first_class_map(&type_defs, &[], &e2e_config, &call);
    let out = render_for("make_opaque", "payload", &type_defs, &functions, &e2e_config, &map);
    assert!(
        out.contains("result.payload()"),
        "opaque result's leaf must keep method-call syntax, got:\n{out}"
    );
}
