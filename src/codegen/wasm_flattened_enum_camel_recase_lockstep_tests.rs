//! Lockstep guard: for a fully-flattened internally-tagged enum's `JsValue` field boundary, the
//! wasm backend's emitted RUNTIME conversion and its declared `.d.ts` SHAPE must move together.
//!
//! `ts_union::tests`'s `flattened_internal_enum_*` tests already pin the declaration half in
//! isolation, and `conversions::core_to_binding::fields::wasm_camel_recase_tests` /
//! `conversions::binding_to_core::fields::wasm_camel_recase_tests` already pin the conversion
//! string-building logic in isolation, against a hand-built `ConversionConfig`. Neither proves
//! that `backends::wasm::gen_bindings::mod`'s real `generate_bindings` run actually WIRES
//! `ConversionConfig::wasm_camel_recased_enums` from its own `JsonWireTypes` registration -- that
//! is exactly the asymmetry `codegen::untagged_enum_wire_cross_backend_tests` was written to
//! catch for a different enum shape, and this module follows the same placement/shape for this
//! one: one real `WasmBackend::generate_bindings` run, one test asserting BOTH the emitted `From`
//! impl and the emitted `.d.ts` custom section in a single body, so a change that moves only one
//! side fails here even though every narrower, hand-built-config unit test still passes.

use crate::core::backend::Backend;
use crate::core::config::ResolvedCrateConfig;
use crate::core::ir::{
    ApiSurface, EnumDef, EnumVariant, FieldDef, FunctionDef, ParamDef, PrimitiveType, TypeDef, TypeRef,
};

use crate::backends::wasm::WasmBackend;

/// `#[serde(tag = "format_type")] enum FormatMetadata { Excel(ExcelMetadata) }` -- the newtype
/// payload flattens into the tag object, so `is_fully_flattened_internal_enum` claims it and it
/// crosses the wasm `JsValue` boundary through the camel-recasing wire-type pipeline rather than
/// a plain `serde_wasm_bindgen` passthrough.
fn format_metadata_enum() -> EnumDef {
    EnumDef {
        name: "FormatMetadata".to_string(),
        rust_path: "test_lib::FormatMetadata".to_string(),
        serde_tag: Some("format_type".to_string()),
        has_serde: true,
        variants: vec![EnumVariant {
            name: "Excel".to_string(),
            is_tuple: true,
            fields: vec![FieldDef {
                name: "_0".to_string(),
                ty: TypeRef::Named("ExcelMetadata".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn excel_metadata_type() -> TypeDef {
    TypeDef {
        name: "ExcelMetadata".to_string(),
        rust_path: "test_lib::ExcelMetadata".to_string(),
        has_serde: true,
        fields: vec![FieldDef {
            name: "sheet_count".to_string(),
            ty: TypeRef::Primitive(PrimitiveType::U32),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// A struct holding the enum as a required field, mirroring a real `DocumentResult { format:
/// FormatMetadata, .. }`.
fn document_result_type() -> TypeDef {
    TypeDef {
        name: "DocumentResult".to_string(),
        rust_path: "test_lib::DocumentResult".to_string(),
        has_serde: true,
        fields: vec![FieldDef {
            name: "format".to_string(),
            ty: TypeRef::Named("FormatMetadata".to_string()),
            optional: false,
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// A free function taking `DocumentResult` by value, so `input_type_names` treats it as an input
/// type and the binding->core `From` impl (the JS -> core half of the lockstep) is actually
/// emitted rather than skipped as dead code -- mirrors
/// `backends::wasm::gen_bindings::untagged_enum_tests::function_taking`.
fn function_taking_document_result() -> FunctionDef {
    FunctionDef {
        name: "use_document_result".to_string(),
        rust_path: "test_lib::use_document_result".to_string(),
        params: vec![ParamDef {
            name: "value".to_string(),
            ty: TypeRef::Named("DocumentResult".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn fixture_api() -> ApiSurface {
    ApiSurface {
        crate_name: "test-lib".to_string(),
        version: "0.1.0".to_string(),
        enums: vec![format_metadata_enum()],
        types: vec![excel_metadata_type(), document_result_type()],
        functions: vec![function_taking_document_result()],
        ..ApiSurface::default()
    }
}

fn generated_wasm_source() -> String {
    WasmBackend
        .generate_bindings(&fixture_api(), &ResolvedCrateConfig::default())
        .expect("wasm backend must generate bindings for the fixture")
        .iter()
        .map(|f| f.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The lockstep assertion itself. See the module doc comment for why this must be ONE test body
/// rather than two separate ones: a regression that reverts only the conversion, or only the
/// declaration, must fail THIS test either way.
#[test]
fn recased_enum_field_conversion_and_declaration_move_together() {
    let source = generated_wasm_source();
    assert!(
        !source.is_empty(),
        "PROBE: the wasm backend must emit something for the fixture"
    );

    // --- Runtime half: the emitted `From` impl for `DocumentResult` must route `format` through
    // the camel wire-type pipeline, not a bare `serde_wasm_bindgen::to_value`/`from_value` on the
    // raw core enum. `__alef_wire_retag_wasm` only appears when the pipeline is actually used --
    // see `codegen::conversions::core_to_binding::fields::camel_jsvalue` /
    // `codegen::conversions::binding_to_core::fields::camel_core_value`.
    assert!(
        source.contains("serde_wasm_bindgen::to_value(&__alef_wire_retag_wasm(serde_json::to_value(&val.format)"),
        "core->binding conversion for `format` must route through the retag+wire-type pipeline, \
         not a bare serde_wasm_bindgen::to_value on the raw core enum; actual source:\n{source}"
    );
    assert!(
        source.contains("serde_json::from_value::<__AlefWireInWasmFormatMetadata>("),
        "binding->core conversion for `format` must decode through the wire-type mirror; actual \
         source:\n{source}"
    );

    // --- Declaration half: the `.d.ts` custom section for this enum must declare camelCase keys.
    assert!(
        source.contains("formatType:"),
        "declared discriminant key must be camelCase; actual source:\n{source}"
    );
    assert!(
        source.contains("sheetCount:"),
        "declared payload field must be camelCase; actual source:\n{source}"
    );
    // Precise TS-declaration-shaped patterns, not a bare `sheet_count:`/`format_type:` substring:
    // the generated Rust source legitimately contains `sheet_count: u32,` (the wasm-bindgen
    // struct's own Rust field) and `"format_type"` (the retag helper's own string-literal
    // argument) for entirely correct, unrelated reasons -- a bare substring check would false-
    // positive on those and fail even when the declaration is genuinely all camelCase.
    assert!(
        !source.contains("sheet_count: number"),
        "must not declare the snake_case TS field type the runtime no longer produces; actual \
         source:\n{source}"
    );
    assert!(
        !source.contains("{ format_type: \""),
        "must not declare the snake_case TS discriminant literal the runtime no longer produces; \
         actual source:\n{source}"
    );
}
