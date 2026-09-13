//! Regression coverage for the Swift e2e generator resolving a fixture's Swift root type from
//! the IR, not only from an explicit `result_type` cross-language override.
//!
//! `render_test_method` derives the resolver's Swift root type from
//! `swift_call_result_type(call_config)` -- a lookup across the `c`/`csharp`/`java`/`kotlin`/
//! `go`/`php` override tables for an explicit `result_type` string -- with no fallback to the
//! call's actual declared Rust return type. Every OTHER per-call anchor built in the same
//! function (`ir_enum_map`, `ir_collection_map`, `ir_result_field_map`) is instead anchored via
//! `call_ir::resolve_declared_result_type`, which reads the function's real IR return type and
//! needs no override at all.
//!
//! As long as a call's return type stays classified opaque in `SwiftFirstClassMap`
//! (`build_swift_first_class_map`), an unresolved Swift root is invisible: `is_first_class(None)`
//! and `is_first_class(Some(opaque_type))` both answer `false`, so every segment renders
//! method-call syntax either way. The gap becomes a real compile break the moment the return
//! type is classified first-class (every field on it primitive/String/Named-first-class/Vec of
//! those) -- the binding emitter starts emitting `public let` stored properties for it, while
//! this resolver, never having learned the call's root type, keeps emitting `.field()`
//! method-call accessors against a struct that has no such method. This is exactly the
//! `ProcessResult.language` shape a peer report described: a plain `String` leaf on a
//! promoted-to-first-class root type, with no explicit `result_type` override configured for the
//! call at all -- the common case.
use crate::core::config::ResolvedCrateConfig;
use crate::core::ir::{FieldDef, FunctionDef, TypeDef, TypeRef};
use crate::e2e::config::{CallConfig, E2eConfig};
use crate::e2e::fixture::{Assertion, Fixture};

fn process_result_ir() -> (Vec<TypeDef>, Vec<FunctionDef>) {
    let type_defs = vec![TypeDef {
        name: "ProcessResult".to_string(),
        has_serde: true,
        fields: vec![FieldDef {
            name: "language".to_string(),
            ty: TypeRef::String,
            ..FieldDef::default()
        }],
        ..TypeDef::default()
    }];
    let functions = vec![FunctionDef {
        name: "process".to_string(),
        return_type: TypeRef::Named("ProcessResult".to_string()),
        ..FunctionDef::default()
    }];
    (type_defs, functions)
}

fn render(type_defs: &[TypeDef], functions: &[FunctionDef]) -> String {
    let call_config = CallConfig {
        function: "process".to_string(),
        result_var: "result".to_string(),
        ..CallConfig::default()
    };
    let mut e2e_config = E2eConfig::default();
    e2e_config.calls.insert("process".to_string(), call_config);
    let call = e2e_config.calls["process"].clone();
    let swift_first_class_map = super::values::build_swift_first_class_map(type_defs, &[], &e2e_config, &call);

    let fixture = Fixture {
        id: "process_language".to_string(),
        description: "language leaf on a promoted root".to_string(),
        call: Some("process".to_string()),
        assertions: vec![Assertion {
            assertion_type: "equals".to_string(),
            field: Some("language".to_string()),
            value: Some(serde_json::Value::String("c".to_string())),
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
        &e2e_config,
        "process",
        "result",
        &[],
        false,
        None,
        &swift_first_class_map,
        "Sample",
        &config,
        type_defs,
        &[],
        functions,
        &[],
    );
    out
}

/// Control: `build_swift_first_class_map` must classify `ProcessResult` as first-class on its
/// own terms (a single `String` field is trivially eligible) before this test can say anything
/// about the resolver that consumes it.
#[test]
fn the_classifier_promotes_a_single_string_field_result_type_to_first_class() {
    let (type_defs, functions) = process_result_ir();
    let call_config = CallConfig {
        function: "process".to_string(),
        ..CallConfig::default()
    };
    let mut e2e_config = E2eConfig::default();
    e2e_config.calls.insert("process".to_string(), call_config.clone());
    let map = super::values::build_swift_first_class_map(&type_defs, &[], &e2e_config, &call_config);
    assert!(
        map.is_first_class(Some("ProcessResult")),
        "control failed: ProcessResult must be classified first-class by its own fields"
    );
    let _ = functions;
}

/// A fixture calling `process` (returns `ProcessResult`, first-class, no `result_type` override
/// configured for any language) must render its `language` leaf with Swift property syntax
/// (`result.language`), never method-call syntax (`result.language()`) — the latter does not
/// compile against a `public let language: String` stored property.
#[test]
fn a_plain_string_leaf_on_an_unoverridden_first_class_root_renders_property_syntax() {
    let (type_defs, functions) = process_result_ir();
    let out = render(&type_defs, &functions);
    assert!(
        out.contains("XCTAssertEqual(result.language, \"c\")"),
        "expected property-syntax access to `language` (no parens), got:\n{out}"
    );
    assert!(
        !out.contains("result.language()"),
        "must not emit method-call syntax against a first-class stored property, got:\n{out}"
    );
}
