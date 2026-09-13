//! Cross-generator parity between the compiled `#[napi]` runtime shape ([`gen_enum`], via
//! [`super::gen_tagged_enum_as_object`]) and the declared `.d.ts` shape (`errors::gen_dts`) for
//! the SAME `EnumDef`.
//!
//! An enum with no explicit `#[serde(tag/content/untagged)]` but with a data-carrying variant is
//! the exact shape that regressed: `enums::gen_enum` (the single authority, see
//! [`super::is_tagged_data_enum`]) has always routed it to the tagged-OBJECT emitter, because a
//! `#[napi(string_enum)]` cannot hold a payload. `errors::gen_dts`'s `Decl::Enum` dispatch used to
//! re-derive the same routing decision locally as `e.serde_tag.is_some()`, which is strictly
//! narrower -- it never covers this shape -- so the `.d.ts` declared a plain string enum for a
//! type the compiled extension actually returns as `{ type: "...", ... }`. `tsc` cannot catch
//! this: the declaration and a snippet checked against it agree with each other and both disagree
//! with the runtime value. Only running the generated test fails, and it fails the same way every
//! time (`String({type:"Function",...}) === "[object Object]"`).
//!
//! These tests call the two real production entry points -- [`gen_enum`] and
//! `errors::gen_dts` -- on one shared `EnumDef` fixture and assert their exact rendered output,
//! so a future re-introduction of a second, narrower shape derivation on either side fails here
//! instead of shipping.

use super::gen_enum;
use crate::backends::napi::gen_bindings::errors::gen_dts;
use crate::core::ir::{ApiSurface, EnumDef, EnumVariant, FieldDef, TypeDef, TypeRef};

/// A default-representation (no `serde_tag`, no `serde_content`, not `serde_untagged`) enum with
/// one data-carrying tuple variant and one unit variant -- the shape that must route through the
/// tagged-object emitter on both the runtime and `.d.ts` sides, even without an explicit
/// `#[serde(tag = "...")]`.
fn sample_kind_enum() -> EnumDef {
    EnumDef {
        name: "SampleKind".to_string(),
        rust_path: "test_core::SampleKind".to_string(),
        variants: vec![
            EnumVariant {
                name: "Function".to_string(),
                is_tuple: true,
                fields: vec![FieldDef {
                    name: "_0".to_string(),
                    ty: TypeRef::String,
                    ..Default::default()
                }],
                ..Default::default()
            },
            EnumVariant {
                name: "Idle".to_string(),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

/// The compiled `#[napi]` runtime shape for [`sample_kind_enum`] is a tagged OBJECT, not a
/// `#[napi(string_enum)]` -- pins the runtime half of the parity this module protects.
#[test]
fn runtime_struct_is_tagged_object_for_default_tagged_data_enum() {
    let enum_def = sample_kind_enum();
    let runtime = gen_enum(&enum_def, "Js", false, "test_core", None, &[]);

    let expected = "\
#[derive(Clone)]
#[napi(object, js_name = \"SampleKind\")]
pub struct JsSampleKind {
    #[napi(js_name = \"type\")]
    pub type_tag: String,
    pub function: Option<String>,
}

impl Default for JsSampleKind {
    fn default() -> Self { Self { type_tag: \"Function\".to_string(), function: None } }
}";
    assert_eq!(runtime, expected);
}

/// The generated `.d.ts` declaration for [`sample_kind_enum`] is a discriminated union of objects
/// -- the TypeScript description of the exact struct [`runtime_struct_is_tagged_object_for_default_tagged_data_enum`]
/// pins -- never a plain `export declare enum`. This is the regression this module exists to
/// catch: before the fix, `errors::gen_dts` re-derived its own narrower "is this a tagged data
/// enum" check (`e.serde_tag.is_some()`) instead of asking the same authority `gen_enum` uses, so
/// this exact fixture declared `export declare enum SampleKind { Function = "Function", Idle =
/// "Idle" }` -- a string enum -- while the runtime returned an object. `find(...).expect(...)`
/// fails loudly if that regresses, rather than silently comparing against the wrong block.
#[test]
fn dts_declaration_is_discriminated_union_for_default_tagged_data_enum() {
    let enum_def = sample_kind_enum();
    let api = ApiSurface {
        enums: vec![enum_def],
        ..Default::default()
    };
    let dts = gen_dts(
        &api,
        "Js",
        &Default::default(),
        &[],
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        "",
        None,
    );

    let start = dts
        .find("export type SampleKind =")
        .expect("gen_dts must declare SampleKind as a discriminated union type, not a plain enum");
    let declaration = dts[start..].trim_end();

    assert_eq!(
        declaration,
        "export type SampleKind =\n  | { type: 'Function'; function: string }\n  | { type: 'Idle' }"
    );
}

/// The runtime struct's discriminant `js_name` and payload field name must literally match the
/// keys the `.d.ts` union declares for the same fixture -- computed from each side's real output
/// (not two independently hand-typed literals), so this fails if either generator's naming ever
/// drifts from the other even when both still agree on "object, not enum".
#[test]
fn dts_and_runtime_agree_on_discriminant_and_payload_field_names() {
    let enum_def = sample_kind_enum();

    let runtime = gen_enum(&enum_def, "Js", false, "test_core", None, &[]);
    let runtime_tag_line = runtime
        .lines()
        .find(|l| l.trim_start().starts_with("#[napi(js_name ="))
        .expect("runtime struct must declare a js_name for its discriminant field");
    assert_eq!(runtime_tag_line.trim(), "#[napi(js_name = \"type\")]");
    let runtime_payload_line = runtime
        .lines()
        .find(|l| l.trim_start().starts_with("pub function:"))
        .expect("runtime struct must declare the Function variant's payload field");
    assert_eq!(runtime_payload_line.trim(), "pub function: Option<String>,");

    let api = ApiSurface {
        enums: vec![enum_def],
        ..Default::default()
    };
    let dts = gen_dts(
        &api,
        "Js",
        &Default::default(),
        &[],
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        "",
        None,
    );
    let start = dts
        .find("export type SampleKind =")
        .expect("gen_dts must declare SampleKind as a discriminated union type, not a plain enum");
    let function_member = dts[start..]
        .lines()
        .find(|l| l.trim_start().starts_with("| { type: 'Function';"))
        .expect(".d.ts must declare the Function variant's member shape");
    assert_eq!(function_member.trim(), "| { type: 'Function'; function: string }");
}

/// A fully flattened internally-tagged enum -- EVERY data-carrying variant's single tuple
/// payload resolves to a `Named` struct and therefore flattens (see
/// `super::is_fully_flattened_internal_enum`). Unlike [`sample_kind_enum`], `gen_enum` cannot
/// emit this as one `#[napi(object)]` struct at all (different variants can want the same field
/// name at different Rust types), so both sides route through the JSON-passthrough wrapper
/// instead of the tagged-object emitter.
fn fully_flattened_format_metadata_enum() -> EnumDef {
    EnumDef {
        name: "FormatMetadata".to_string(),
        rust_path: "test_core::FormatMetadata".to_string(),
        serde_tag: Some("format_type".to_string()),
        serde_rename_all: Some("snake_case".to_string()),
        variants: vec![
            EnumVariant {
                name: "Excel".to_string(),
                is_tuple: true,
                fields: vec![FieldDef {
                    name: "_0".to_string(),
                    ty: TypeRef::Named("ExcelMetadata".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            EnumVariant {
                name: "Csv".to_string(),
                is_tuple: true,
                fields: vec![FieldDef {
                    name: "_0".to_string(),
                    ty: TypeRef::Named("CsvMetadata".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

fn fully_flattened_format_metadata_types() -> Vec<TypeDef> {
    vec![
        TypeDef {
            name: "ExcelMetadata".to_string(),
            rust_path: "test_core::ExcelMetadata".to_string(),
            fields: vec![FieldDef {
                name: "sheet_count".to_string(),
                ty: TypeRef::Primitive(crate::core::ir::PrimitiveType::U32),
                ..Default::default()
            }],
            ..Default::default()
        },
        TypeDef {
            name: "CsvMetadata".to_string(),
            rust_path: "test_core::CsvMetadata".to_string(),
            fields: vec![FieldDef {
                name: "delimiter".to_string(),
                ty: TypeRef::String,
                ..Default::default()
            }],
            ..Default::default()
        },
    ]
}

/// Runtime half: a fully flattened enum must compile as the `serde_json::Value` passthrough
/// wrapper, never a `#[napi(object)]` struct.
#[test]
fn runtime_is_json_passthrough_for_fully_flattened_enum() {
    let enum_def = fully_flattened_format_metadata_enum();
    let types = fully_flattened_format_metadata_types();
    let runtime = gen_enum(&enum_def, "Js", true, "test_core", None, &types);

    assert!(
        runtime.contains("pub struct JsFormatMetadata(pub serde_json::Value)"),
        "must route through the JSON passthrough wrapper; got:\n{runtime}"
    );
    assert!(!runtime.contains("#[napi(object"), "got:\n{runtime}");
}

/// `.d.ts` half: the declared union must use serde's OWN field names (never napi's camelCase
/// `js_name` renaming, since there is no nominal napi struct to rename fields on), and no
/// `export declare enum` fallback. `WireTypes` derives this shape generically for
/// `SerdeEnumRepr::Internal`, the same helper the container-level-untagged branch already reuses,
/// so this pins that reuse rather than a hand-written second copy.
#[test]
fn dts_declaration_uses_serde_field_names_for_fully_flattened_enum() {
    let enum_def = fully_flattened_format_metadata_enum();
    let api = ApiSurface {
        enums: vec![enum_def],
        types: fully_flattened_format_metadata_types(),
        ..Default::default()
    };
    let dts = gen_dts(
        &api,
        "Js",
        &Default::default(),
        &[],
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        "",
        None,
    );

    let start = dts
        .find("export type FormatMetadata =")
        .expect("gen_dts must declare FormatMetadata as a union type, not a plain enum");
    let declaration_block = &dts[start..];

    assert!(
        declaration_block.contains("format_type: \"excel\""),
        "the tag key must be the enum's own serde_tag, not napi's synthesized 'kind'; got:\n{dts}"
    );
    assert!(declaration_block.contains("format_type: \"csv\""), "got:\n{dts}");
    // Serde's own (snake_case) field names, never a camelCase `js_name` rename -- there is no
    // nominal napi struct here to rename fields on. ~keep
    assert!(dts.contains("sheet_count: number"), "got:\n{dts}");
    assert!(dts.contains("delimiter: string"), "got:\n{dts}");
    assert!(
        !dts.contains("export declare enum FormatMetadata"),
        "must never fall back to a plain string enum; got:\n{dts}"
    );
}
