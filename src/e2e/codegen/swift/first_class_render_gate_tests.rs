//! Structural gate: no generated Swift e2e test emits method-call `()` syntax, or a member a
//! first-class type does not declare, against a leaf whose owner type is in the Swift first-class
//! (promoted) set — and separately, that a payload-carrying promoted enum leaf still renders a
//! REAL assertion rather than a silently-dropped skip.
//!
//! ~keep This is deliberately a DIFFERENT invariant than
//! `first_class_classifier_parity_tests.rs`. Parity asks "do the binding classifier and the e2e
//! classifier AGREE on which types are promoted" — it says nothing about whether a renderer that
//! never asks either classifier still emits opaque syntax against an agreed-promoted type. Three
//! such renderers did exactly that: `swift_traversal_contains_assert` and
//! `render_wildcard_assertion`'s `not_empty` arm each unconditionally appended `.toString()` to a
//! wildcard element accessor regardless of whether that accessor was already bare (first-class)
//! property syntax, and `stringy_field_text_line`'s `contains`-aggregator unconditionally emitted
//! `item.field()` method-call syntax for every text-bearing accessor on a `Vec<T>` element type,
//! never checking `SwiftFirstClassMap::is_first_class` at all. All three compiled clean tests
//! while generating Swift that failed to compile downstream — the gap parity cannot see, because
//! both classifiers were never even consulted. This gate scans the RENDERED Swift text itself for
//! the three known first-class field names below, so it fails on any renderer (present or future)
//! that regresses this, not just the three fixed here.
//!
//! ~keep The FIRST fix attempt made every payload-carrying enum leaf render a `// skipped:` line
//! instead of a real assertion. That satisfies "no unavailable member is referenced" while
//! quietly deleting the check — the exact regression this codebase has a name for ("a previous
//! Swift fix turned the suite green by DELETING 8 assertions; the lesson was count XCTAssert
//! lines, not compile status"). `gen_bindings::enums::emit_swift_wire_tag_accessor` now gives a
//! promoted payload-carrying enum its own `toString()` (the same serde wire tag the pre-promotion
//! opaque mirror's `to_string()` always returned), so the accessor is genuinely available and the
//! skip is no longer the right answer. `payload_carrying_enum_leaf_renders_a_real_assertion_not_a_skip`
//! below is what proves this gate cannot be satisfied by silently dropping coverage again.

use crate::core::config::ResolvedCrateConfig;
use crate::core::ir::{EnumDef, EnumVariant, FieldDef, FunctionDef, TypeDef, TypeRef};
use crate::e2e::config::{CallConfig, CallOverride, E2eConfig};
use crate::e2e::field_access::SwiftFirstClassMap;
use crate::e2e::fixture::{Assertion, Fixture};
use std::collections::{HashMap, HashSet};

/// An all-unit enum (two fieldless variants) — the ONLY shape
/// `gen_bindings::enums::emit_enum` gives a `: String` raw value, so `gateKind` must lower
/// through `.rawValue`, never `.toString()`.
fn unit_tag_enum() -> EnumDef {
    EnumDef {
        name: "UnitTag".to_string(),
        variants: vec![
            EnumVariant {
                name: "Alpha".to_string(),
                ..EnumVariant::default()
            },
            EnumVariant {
                name: "Beta".to_string(),
                ..EnumVariant::default()
            },
        ],
        has_serde: true,
        ..EnumDef::default()
    }
}

/// A payload-carrying enum (one tuple variant). Once promoted it declares no `.rawValue` (only
/// the all-unit shape gets one), but DOES declare `.toString()`
/// (`gen_bindings::enums::emit_swift_wire_tag_accessor`) — so `gatePayload` must render a real
/// assertion through `.toString()`, never a skip and never `.rawValue`.
fn payload_tag_enum() -> EnumDef {
    EnumDef {
        name: "PayloadTag".to_string(),
        variants: vec![EnumVariant {
            name: "Text".to_string(),
            fields: vec![FieldDef {
                name: "_0".to_string(),
                ty: TypeRef::String,
                ..FieldDef::default()
            }],
            is_tuple: true,
            ..EnumVariant::default()
        }],
        has_serde: true,
        ..EnumDef::default()
    }
}

/// `GateItem { kind: UnitTag, payload: PayloadTag, label: String }`, the element type of
/// `GateResult.items`. Three leaves, one of each shape this gate exists to police.
fn gate_item_type() -> TypeDef {
    TypeDef {
        name: "GateItem".to_string(),
        fields: vec![
            FieldDef {
                name: "kind".to_string(),
                ty: TypeRef::Named("UnitTag".to_string()),
                ..FieldDef::default()
            },
            FieldDef {
                name: "payload".to_string(),
                ty: TypeRef::Named("PayloadTag".to_string()),
                ..FieldDef::default()
            },
            FieldDef {
                name: "label".to_string(),
                ty: TypeRef::String,
                ..FieldDef::default()
            },
        ],
        ..TypeDef::default()
    }
}

fn gate_result_type() -> TypeDef {
    TypeDef {
        name: "GateResult".to_string(),
        fields: vec![FieldDef {
            name: "items".to_string(),
            ty: TypeRef::Vec(Box::new(TypeRef::Named("GateItem".to_string()))),
            ..FieldDef::default()
        }],
        ..TypeDef::default()
    }
}

/// A `SwiftFirstClassMap` that treats BOTH `GateResult` and `GateItem` as first-class Codable
/// structs — property syntax all the way down the `items[].kind` / `items[].payload` /
/// `items[].label` chain, exactly the shape that exposed all three bugs (a wildcard element
/// pulled from a `RustVec<GateItem>` stays first-class here on purpose: unlike a swift-bridge
/// `RustVec`, a first-class `[GateItem]` is a plain Swift array whose subscript yields `GateItem`
/// directly).
fn first_class_map() -> SwiftFirstClassMap {
    let mut field_types = HashMap::new();
    field_types.insert(
        "GateResult".to_string(),
        HashMap::from([("items".to_string(), "GateItem".to_string())]),
    );
    let mut stringy_fields_by_type = HashMap::new();
    stringy_fields_by_type.insert(
        "GateItem".to_string(),
        vec![
            crate::e2e::field_access::StringyField {
                name: "kind".to_string(),
                kind: crate::e2e::field_access::StringyFieldKind::Plain,
            },
            crate::e2e::field_access::StringyField {
                name: "payload".to_string(),
                kind: crate::e2e::field_access::StringyFieldKind::Plain,
            },
            crate::e2e::field_access::StringyField {
                name: "label".to_string(),
                kind: crate::e2e::field_access::StringyFieldKind::Plain,
            },
        ],
    );
    SwiftFirstClassMap {
        first_class_types: ["GateResult", "GateItem"].into_iter().map(str::to_string).collect(),
        field_types,
        vec_field_names: HashSet::from(["items".to_string()]),
        json_bridged_field_names: HashSet::new(),
        json_bridged_by_type: HashMap::new(),
        getter_optionality: HashMap::new(),
        root_type: Some("GateResult".to_string()),
        stringy_fields_by_type,
    }
}

fn e2e_config() -> E2eConfig {
    let mut call_config = CallConfig {
        function: "gate".to_string(),
        ..CallConfig::default()
    };
    call_config.overrides.insert(
        "csharp".to_string(),
        CallOverride {
            result_type: Some("GateResult".to_string()),
            ..CallOverride::default()
        },
    );
    let mut e2e_config = E2eConfig::default();
    e2e_config.calls.insert("gate".to_string(), call_config);
    e2e_config
}

/// The same shape with the ROOT demoted: `GateResult` stays a `typealias` to the opaque
/// `RustBridge.GateResult` while `GateItem` is still promoted. This is exactly crawlberg's
/// `CrawlResult.cookies` — `CookieInfo` is a first-class Codable struct, but the value reached
/// through the opaque root's `cookies()` getter is a `RustVec<RustBridge.CookieInfo>`, whose
/// elements only have swift-bridge METHOD accessors. Element promotion alone must not decide
/// the syntax. ~keep
fn opaque_root_map() -> SwiftFirstClassMap {
    let mut map = first_class_map();
    map.first_class_types.remove("GateResult");
    map
}

fn render(fixture: &Fixture) -> String {
    render_with_map(fixture, first_class_map())
}

fn render_with_map(fixture: &Fixture, map: SwiftFirstClassMap) -> String {
    let type_defs = [gate_result_type(), gate_item_type()];
    let enums = [unit_tag_enum(), payload_tag_enum()];
    let functions = [FunctionDef {
        name: "gate".to_string(),
        return_type: TypeRef::Named("GateResult".to_string()),
        ..FunctionDef::default()
    }];
    let e2e_config = e2e_config();
    let config = ResolvedCrateConfig {
        name: "sample".to_string(),
        ..ResolvedCrateConfig::default()
    };
    let mut out = String::new();
    super::test_method::render_test_method(
        &mut out,
        fixture,
        &e2e_config,
        "",
        "",
        &[],
        false,
        None,
        &map,
        "Sample",
        &config,
        &type_defs,
        &enums,
        &functions,
        &[],
    );
    out
}

fn wildcard_fixture(id: &str, assertion_type: &str, field: &str, value: Option<&str>) -> Fixture {
    Fixture {
        id: id.to_string(),
        description: id.to_string(),
        call: Some("gate".to_string()),
        assertions: vec![Assertion {
            assertion_type: assertion_type.to_string(),
            field: Some(field.to_string()),
            value: value.map(|v| serde_json::Value::String(v.to_string())),
            ..Assertion::default()
        }],
        ..Fixture::default()
    }
}

/// Every string that would prove a renderer fell back to opaque (method-call) syntax against a
/// first-class leaf, or referenced a member the leaf's real Swift type does not declare.
/// `label` (`String`) has neither `.toString()` nor `()`. `kind` (`UnitTag`) has `.rawValue` but
/// no `.toString()` or `()`. `payload` (`PayloadTag`) has `.toString()`
/// (`emit_swift_wire_tag_accessor`) but no `()` and no `.rawValue`.
const FORBIDDEN: &[&str] = &[
    "kind()",
    "kind.toString()",
    "label()",
    "label.toString()",
    "payload()",
    "payload.rawValue",
];

fn assert_gate_holds(out: &str, scenario: &str) {
    for needle in FORBIDDEN {
        assert!(
            !out.contains(needle),
            "{scenario}: rendered Swift references a first-class leaf via opaque or unavailable \
             syntax (found {needle:?}), got:\n{out}"
        );
    }
}

/// Class 1 + 2 (wildcard `contains`): `items[].kind` (unit enum) must lower through `.rawValue`,
/// `items[].label` (plain `String`) must stay bare, and `items[].payload` (payload-carrying enum)
/// must lower through its own `.toString()` — a real assertion, not a skip.
#[test]
fn wildcard_contains_never_emits_opaque_or_unavailable_syntax() {
    for field in ["items[].kind", "items[].label", "items[].payload"] {
        let fixture = wildcard_fixture(&format!("contains_{field}"), "contains", field, Some("x"));
        let out = render(&fixture);
        assert_gate_holds(&out, &format!("wildcard contains on '{field}'"));
    }
    let payload_out = render(&wildcard_fixture(
        "contains_payload",
        "contains",
        "items[].payload",
        Some("x"),
    ));
    assert!(
        payload_out.contains("XCTAssertTrue") && payload_out.contains("payload.toString()"),
        "a payload-carrying union element must lower through its own .toString(), got:\n{payload_out}"
    );
    assert!(
        !payload_out.contains("skipped:"),
        "a payload-carrying union element has a real accessor now — it must not be skipped, got:\n{payload_out}"
    );
    let kind_out = render(&wildcard_fixture(
        "contains_kind",
        "contains",
        "items[].kind",
        Some("x"),
    ));
    assert!(
        kind_out.contains("kind.rawValue"),
        "a first-class unit enum element must still lower through .rawValue, got:\n{kind_out}"
    );
    let label_out = render(&wildcard_fixture(
        "contains_label",
        "contains",
        "items[].label",
        Some("x"),
    ));
    assert!(
        label_out.contains("$0.label"),
        "a first-class String element must stay bare property access, got:\n{label_out}"
    );
}

/// Class 1 + 2 (wildcard `not_empty`, the arm that builds its own element accessor inline rather
/// than going through `swift_traversal_contains_assert`).
#[test]
fn wildcard_not_empty_never_emits_opaque_or_unavailable_syntax() {
    for field in ["items[].kind", "items[].label", "items[].payload"] {
        let fixture = wildcard_fixture(&format!("not_empty_{field}"), "not_empty", field, None);
        let out = render(&fixture);
        assert_gate_holds(&out, &format!("wildcard not_empty on '{field}'"));
        assert!(
            !out.contains("skipped:"),
            "not_empty on '{field}' has a real accessor available — it must not be skipped, got:\n{out}"
        );
    }
}

/// Class 3 (the stringy-field `contains` aggregator over `items`, which has no per-element field
/// path — `swift_stringy_aggregator_contains_assert` walks every text-bearing accessor on
/// `GateItem` itself).
#[test]
fn stringy_aggregator_never_emits_opaque_or_unavailable_syntax() {
    let fixture = wildcard_fixture("aggregator", "contains", "items", Some("x"));
    let out = render(&fixture);
    assert_gate_holds(&out, "stringy aggregator over 'items'");
    // All three fields must now contribute — including `payload`, via its own `.toString()`.
    for needle in [
        "texts.append(item.label)",
        "texts.append(item.kind.rawValue)",
        "texts.append(item.payload.toString())",
    ] {
        assert!(
            out.contains(needle),
            "expected the aggregator to include {needle:?}, got:\n{out}"
        );
    }
}

/// The measurement the FIRST fix attempt could not have passed: a payload-carrying promoted enum
/// leaf must render a REAL `XCTAssertTrue`, not a `// skipped:` comment nobody reads. Checked
/// across every renderer this gate covers (wildcard `contains`, wildcard `not_empty`, and the
/// stringy aggregator), so a regression in any one of them — reverting to the skip — is caught
/// here specifically, independent of the opaque/unavailable-syntax checks above.
#[test]
fn payload_carrying_enum_leaf_renders_a_real_assertion_not_a_skip() {
    let scenarios = [
        (
            "wildcard contains",
            render(&wildcard_fixture(
                "pc_contains",
                "contains",
                "items[].payload",
                Some("x"),
            )),
        ),
        (
            "wildcard not_empty",
            render(&wildcard_fixture("pc_not_empty", "not_empty", "items[].payload", None)),
        ),
        (
            "stringy aggregator",
            render(&wildcard_fixture("pc_aggregator", "contains", "items", Some("x"))),
        ),
    ];
    for (scenario, out) in scenarios {
        assert!(
            !out.contains("skipped:"),
            "{scenario}: a payload-carrying enum leaf has a real .toString() accessor now, it must \
             not render a skip — got:\n{out}"
        );
        assert!(
            out.contains("payload.toString()"),
            "{scenario}: expected the payload-carrying enum leaf to lower through .toString(), got:\n{out}"
        );
        assert!(
            out.contains("XCTAssertTrue"),
            "{scenario}: expected a real XCTAssertTrue, got:\n{out}"
        );
    }
}

/// Class 3 with an OPAQUE root: the aggregator must fall back to method-call syntax for every
/// stringy field, because `result.items()` on a `typealias`-to-`RustBridge` root yields a
/// `RustVec<RustBridge.GateItem>` — first-class promotion of `GateItem` is irrelevant there.
/// Regression for crawlberg's `CookiesTests.swift` (alef 0.87.0 → `texts.append(item.name)`
/// against a `CookieInfoRef`, "cannot convert value of type '() -> RustString' to 'String'").
#[test]
fn stringy_aggregator_uses_method_syntax_when_root_is_opaque() {
    let fixture = wildcard_fixture("aggregator_opaque_root", "contains", "items", Some("x"));
    let out = render_with_map(&fixture, opaque_root_map());
    assert!(
        out.contains("result.items().contains(where:"),
        "expected the array accessor on an opaque root to be a method call, got:\n{out}"
    );
    for needle in [
        "texts.append(item.label().toString())",
        "texts.append(item.kind().toString())",
        "texts.append(item.payload().toString())",
    ] {
        assert!(
            out.contains(needle),
            "expected the aggregator to lower every element field as an opaque getter, expected \
             {needle:?}, got:\n{out}"
        );
    }
    for needle in [
        "texts.append(item.label)",
        "item.kind.rawValue",
        "item.payload.toString()",
    ] {
        assert!(
            !out.contains(needle),
            "an element pulled from a RustVec on an opaque root has no properties, found {needle:?} in:\n{out}"
        );
    }
}
