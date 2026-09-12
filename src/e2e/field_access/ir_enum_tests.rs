//! Table-driven tests for `crate::e2e::field_access::ir_enum` and its integration into
//! `FieldResolver::is_enum` — the fix for the defect where enum-ness was decided purely from
//! a hand-written `alef.toml` `fields_enum` list instead of the crate's own IR.

use std::collections::HashSet;

use crate::core::ir::{EnumDef, EnumVariant, FieldDef, TypeDef, TypeRef};
use crate::e2e::field_access::FieldResolver;

use super::ir_enum::{build_ir_enum_map, is_enum_path};
use super::types::IrEnumMap;

fn field(name: &str, ty: TypeRef) -> FieldDef {
    FieldDef {
        name: name.to_string(),
        ty,
        ..FieldDef::default()
    }
}

fn type_def(name: &str, fields: Vec<FieldDef>) -> TypeDef {
    TypeDef {
        name: name.to_string(),
        fields,
        ..TypeDef::default()
    }
}

fn enum_def(name: &str) -> EnumDef {
    EnumDef {
        name: name.to_string(),
        ..EnumDef::default()
    }
}

/// The fixture at the heart of the reported defect: two structs each declare a field named
/// `kind`, but only one of them is actually enum-typed. `DataNode.kind: DataNodeKind` (a real
/// IR enum) sits beside `PlainNode.kind: String`. A name-keyed rule cannot get both right.
fn ambiguous_kind_type_defs() -> Vec<TypeDef> {
    vec![
        type_def(
            "DataNode",
            vec![field("kind", TypeRef::Named("DataNodeKind".to_string()))],
        ),
        type_def("PlainNode", vec![field("kind", TypeRef::String)]),
    ]
}

fn ambiguous_kind_enums() -> Vec<EnumDef> {
    vec![enum_def("DataNodeKind")]
}

#[test]
fn a_field_whose_declared_type_is_a_real_enum_is_derived_as_enum() {
    let map = build_ir_enum_map(&ambiguous_kind_type_defs(), &ambiguous_kind_enums());
    let map = IrEnumMap {
        root_type: Some("DataNode".to_string()),
        ..map
    };

    assert!(is_enum_path(&map, "kind"), "DataNode.kind is DataNodeKind, a real enum");
}

#[test]
fn a_field_with_the_same_name_but_a_string_type_on_a_different_owner_is_not_enum() {
    let map = build_ir_enum_map(&ambiguous_kind_type_defs(), &ambiguous_kind_enums());
    let map = IrEnumMap {
        root_type: Some("PlainNode".to_string()),
        ..map
    };

    assert!(
        !is_enum_path(&map, "kind"),
        "PlainNode.kind is String — the bare name 'kind' must not decide this"
    );
}

#[test]
fn an_option_wrapped_enum_field_is_derived_as_enum() {
    let type_defs = vec![type_def(
        "Response",
        vec![field(
            "status",
            TypeRef::Optional(Box::new(TypeRef::Named("Status".to_string()))),
        )],
    )];
    let enums = vec![enum_def("Status")];
    let map = build_ir_enum_map(&type_defs, &enums);
    let map = IrEnumMap {
        root_type: Some("Response".to_string()),
        ..map
    };

    assert!(is_enum_path(&map, "status"), "Option<Status> must unwrap to the enum");
}

#[test]
fn a_vec_wrapped_element_field_reached_via_wildcard_traversal_is_derived_as_enum() {
    // `Result.links: Vec<Link>`, `Link.link_type: LinkType` (enum) — mirrors the
    // `links[].link_type` path form the Rust wildcard-assertion renderer produces.
    let type_defs = vec![
        type_def(
            "Result",
            vec![field(
                "links",
                TypeRef::Vec(Box::new(TypeRef::Named("Link".to_string()))),
            )],
        ),
        type_def("Link", vec![field("link_type", TypeRef::Named("LinkType".to_string()))]),
    ];
    let enums = vec![enum_def("LinkType")];
    let map = build_ir_enum_map(&type_defs, &enums);
    let map = IrEnumMap {
        root_type: Some("Result".to_string()),
        ..map
    };

    assert!(
        is_enum_path(&map, "links[].link_type"),
        "Vec<Link>.link_type must be reached through the wildcard array segment"
    );
    // The already-split element sub-path (what a hand-written `fields_enum` entry would
    // name) must NOT resolve on its own without the array segment: `link_type` is not a
    // direct field of `Result`, the root type.
    assert!(
        !is_enum_path(&map, "link_type"),
        "a bare leaf name must not resolve against the wrong owner type"
    );
}

#[test]
fn a_nested_indexed_path_is_derived_as_enum() {
    // `Response.choices: Vec<Choice>`, `Choice.finish_reason: FinishReason` (enum) — mirrors
    // `choices[0].finish_reason`.
    let type_defs = vec![
        type_def(
            "Response",
            vec![field(
                "choices",
                TypeRef::Vec(Box::new(TypeRef::Named("Choice".to_string()))),
            )],
        ),
        type_def(
            "Choice",
            vec![field("finish_reason", TypeRef::Named("FinishReason".to_string()))],
        ),
    ];
    let enums = vec![enum_def("FinishReason")];
    let map = build_ir_enum_map(&type_defs, &enums);
    let map = IrEnumMap {
        root_type: Some("Response".to_string()),
        ..map
    };

    assert!(is_enum_path(&map, "choices[0].finish_reason"));
}

#[test]
fn a_missing_root_type_answers_false_rather_than_guessing() {
    let map = build_ir_enum_map(&ambiguous_kind_type_defs(), &ambiguous_kind_enums());
    // root_type left as None (build_ir_enum_map never sets it).
    assert!(!is_enum_path(&map, "kind"));
}

#[test]
fn a_path_through_an_unknown_field_answers_false() {
    let map = build_ir_enum_map(&ambiguous_kind_type_defs(), &ambiguous_kind_enums());
    let map = IrEnumMap {
        root_type: Some("DataNode".to_string()),
        ..map
    };

    assert!(!is_enum_path(&map, "nonexistent_field"));
    assert!(!is_enum_path(&map, "nonexistent_parent.kind"));
}

/// End-to-end proof that `FieldResolver::is_enum` actually consults the IR fallback once
/// `with_ir_enum_map` wires it in — not just the standalone `is_enum_path` helper.
#[test]
fn field_resolver_is_enum_consults_the_ir_fallback_when_config_is_silent() {
    let map = FieldResolver::ir_enum_fields(&ambiguous_kind_type_defs(), &ambiguous_kind_enums());
    let resolver = FieldResolver::new(
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )
    .with_ir_enum_map(map, Some("DataNode".to_string()));

    assert!(
        resolver.is_enum("kind"),
        "fields_enum was never configured; the IR alone must answer this"
    );
}

/// The companion case: the same field name on the type where it is genuinely a `String` must
/// stay `false`, proving the resolver-level integration is exactly as owner-aware as
/// `is_enum_path` itself.
#[test]
fn field_resolver_is_enum_does_not_misclassify_the_same_name_on_a_different_owner() {
    let map = FieldResolver::ir_enum_fields(&ambiguous_kind_type_defs(), &ambiguous_kind_enums());
    let resolver = FieldResolver::new(
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )
    .with_ir_enum_map(map, Some("PlainNode".to_string()));

    assert!(!resolver.is_enum("kind"));
}

/// Hard requirement: an explicitly-configured `fields_enum` entry must keep winning even when
/// the IR would (wrongly, or simply because the config author knows something the IR can't
/// see, e.g. a type alias) disagree — regressing an already-correct consumer config is
/// unacceptable.
#[test]
fn an_explicit_fields_enum_entry_wins_even_when_the_ir_disagrees() {
    let map = FieldResolver::ir_enum_fields(&ambiguous_kind_type_defs(), &ambiguous_kind_enums());
    let resolver = FieldResolver::new(
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )
    .with_enum_fields(HashSet::from(["kind".to_string()]))
    // Anchored at PlainNode, where the IR says `kind` is a plain String.
    .with_ir_enum_map(map, Some("PlainNode".to_string()));

    assert!(
        resolver.is_enum("kind"),
        "an explicit fields_enum entry must win over an IR disagreement"
    );
}

/// A resolver that never calls `with_ir_enum_map` at all (every existing call site before
/// this fix, and every backend that hasn't been wired up yet) must behave exactly as before:
/// `is_enum` answers strictly from `fields_enum`.
#[test]
fn a_resolver_with_no_ir_enum_map_wired_in_behaves_exactly_as_before() {
    let resolver = FieldResolver::new(
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
    );

    assert!(!resolver.is_enum("kind"));

    let resolver = resolver.with_enum_fields(HashSet::from(["kind".to_string()]));
    assert!(resolver.is_enum("kind"));
}

/// `variant_payload_is_collection` must distinguish a tuple variant whose single field is
/// itself `Vec<T>` (`Found(Vec<Entry>)`) from a variant wrapping a struct that merely contains
/// a collection field elsewhere (`Wrapped(Payload)`) — the shape distinction
/// `FieldResolver::union_variant_payload_is_collection` needs when a fixture path names only
/// the variant, with no field inside its payload (the "the payload itself is the list" case
/// `csharp`/`kotlin` count_min assertions used to silently drop).
#[test]
fn variant_payload_is_collection_distinguishes_a_direct_vec_payload_from_a_wrapping_struct() {
    let enums = vec![EnumDef {
        name: "Outcome".to_string(),
        variants: vec![
            EnumVariant {
                name: "Found".to_string(),
                fields: vec![field("_0", TypeRef::Vec(Box::new(TypeRef::Named("Entry".to_string()))))],
                ..EnumVariant::default()
            },
            EnumVariant {
                name: "Wrapped".to_string(),
                fields: vec![field("payload", TypeRef::Named("Payload".to_string()))],
                ..EnumVariant::default()
            },
            EnumVariant {
                name: "Empty".to_string(),
                ..EnumVariant::default()
            },
        ],
        ..EnumDef::default()
    }];
    let map = build_ir_enum_map(&[], &enums);

    assert!(
        map.variant_payload_is_collection
            .get("Outcome")
            .is_some_and(|variants| variants.contains("Found")),
        "Found(Vec<Entry>) wraps a collection directly"
    );
    assert!(
        !map.variant_payload_is_collection
            .get("Outcome")
            .is_some_and(|variants| variants.contains("Wrapped")),
        "Wrapped(Payload) wraps a struct, not a collection"
    );
    assert!(
        !map.variant_payload_is_collection
            .get("Outcome")
            .is_some_and(|variants| variants.contains("Empty")),
        "a fieldless variant has no payload to classify"
    );
}

/// The resolver-level surface `csharp`/`kotlin` call: `union_variant_payload_is_collection`
/// answers `true` for the direct-`Vec` variant and `false` for both the struct-wrapping variant
/// and an unknown union/variant name, without ever needing a field name — unlike
/// `union_variant_field_is_collection`, which requires one and cannot answer this question.
#[test]
fn resolver_union_variant_payload_is_collection_matches_the_ir() {
    let enums = vec![EnumDef {
        name: "Outcome".to_string(),
        variants: vec![
            EnumVariant {
                name: "Found".to_string(),
                fields: vec![field("_0", TypeRef::Vec(Box::new(TypeRef::Named("Entry".to_string()))))],
                ..EnumVariant::default()
            },
            EnumVariant {
                name: "Wrapped".to_string(),
                fields: vec![field("payload", TypeRef::Named("Payload".to_string()))],
                ..EnumVariant::default()
            },
        ],
        ..EnumDef::default()
    }];
    let resolver = FieldResolver::new(
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )
    .with_ir_enum_map(FieldResolver::ir_enum_fields(&[], &enums), None);

    assert!(resolver.union_variant_payload_is_collection("Outcome", "Found"));
    assert!(!resolver.union_variant_payload_is_collection("Outcome", "Wrapped"));
    assert!(!resolver.union_variant_payload_is_collection("Outcome", "Missing"));
    assert!(!resolver.union_variant_payload_is_collection("UnknownUnion", "Found"));
}

/// Scope-boundary control (not a defect): `Vec<Vec<T>>` and `Option<Vec<T>>` payloads both
/// classify as collections through `is_vec_type`'s existing recursion (`Optional` unwraps once,
/// `Vec` matches immediately regardless of its element type), and `named_type` recurses through
/// BOTH layers of `Vec<Vec<T>>` to the same innermost named element `variant_payload_types`
/// already recorded for a single-layer `Vec<T>` -- so the collection-payload classification and
/// the payload type name it records are both anchored on the OUTER `Vec`, which is exactly what
/// `render_bare_variant_payload_assertion`'s `.size`/`.Count` ends up asserting against. Pinned
/// as-is; no production code changed to make this pass. ~keep
#[test]
fn variant_payload_is_collection_covers_nested_vec_and_optional_vec_payloads() {
    let enums = vec![EnumDef {
        name: "Outcome".to_string(),
        variants: vec![
            EnumVariant {
                name: "NestedVec".to_string(),
                fields: vec![field(
                    "_0",
                    TypeRef::Vec(Box::new(TypeRef::Vec(Box::new(TypeRef::Named("Entry".to_string()))))),
                )],
                ..EnumVariant::default()
            },
            EnumVariant {
                name: "OptionalVec".to_string(),
                fields: vec![field(
                    "_0",
                    TypeRef::Optional(Box::new(TypeRef::Vec(Box::new(TypeRef::Named("Entry".to_string()))))),
                )],
                ..EnumVariant::default()
            },
        ],
        ..EnumDef::default()
    }];
    let map = build_ir_enum_map(&[], &enums);

    assert!(
        map.variant_payload_is_collection
            .get("Outcome")
            .is_some_and(|variants| variants.contains("NestedVec")),
        "Vec<Vec<Entry>> classifies as a collection payload via the outer Vec"
    );
    assert_eq!(
        map.variant_payload_types
            .get("Outcome")
            .and_then(|v| v.get("NestedVec")),
        Some(&("_0".to_string(), "Entry".to_string())),
        "named_type recurses through both Vec layers to the innermost named element"
    );

    assert!(
        map.variant_payload_is_collection
            .get("Outcome")
            .is_some_and(|variants| variants.contains("OptionalVec")),
        "Option<Vec<Entry>> classifies as a collection payload via is_vec_type's Optional unwrap"
    );
    assert_eq!(
        map.variant_payload_types
            .get("Outcome")
            .and_then(|v| v.get("OptionalVec")),
        Some(&("_0".to_string(), "Entry".to_string())),
        "named_type unwraps Option then Vec to the same named element"
    );
}

fn variant(name: &str, serde_rename: Option<&str>) -> EnumVariant {
    EnumVariant {
        name: name.to_string(),
        serde_rename: serde_rename.map(str::to_string),
        ..EnumVariant::default()
    }
}

fn wire_variant_enum(rename_all: Option<&str>, variants: Vec<EnumVariant>) -> EnumDef {
    EnumDef {
        name: "Kind".to_string(),
        serde_rename_all: rename_all.map(str::to_string),
        variants,
        ..EnumDef::default()
    }
}

/// Table-driven contract for `enum_wire_variants`, the wire-value -> Rust-identifier reverse
/// lookup a generator needs when it renders an enum on the Rust surface (`{:?}`) but compares
/// against a fixture's serde wire value.
///
/// The map must be populated ONLY where the two spellings genuinely disagree and the answer is
/// unambiguous, because a caller reads a miss as "no rename to reconcile" and keeps the fixture
/// literal verbatim. Recording an entry that is not a real, unique rename would silently
/// rewrite a correct expectation into a different variant's.
#[test]
fn enum_wire_variants_records_only_unambiguous_renames() {
    struct Case {
        name: &'static str,
        rename_all: Option<&'static str>,
        variants: Vec<EnumVariant>,
        lookup: &'static str,
        expected: Option<&'static str>,
    }
    let cases = vec![
        Case {
            name: "explicit serde(rename) maps the wire value back to the identifier",
            rename_all: None,
            variants: vec![variant("KeyValue", Some("key-value"))],
            lookup: "key-value",
            expected: Some("KeyValue"),
        },
        Case {
            name: "rename_all alone is enough to separate the two spellings",
            rename_all: Some("kebab-case"),
            variants: vec![variant("KeyValue", None)],
            lookup: "key-value",
            expected: Some("KeyValue"),
        },
        Case {
            name: "serde(rename) wins over rename_all",
            rename_all: Some("kebab-case"),
            variants: vec![variant("KeyValue", Some("kv"))],
            lookup: "kv",
            expected: Some("KeyValue"),
        },
        Case {
            name: "the rename_all spelling is NOT recorded when serde(rename) overrode it",
            rename_all: Some("kebab-case"),
            variants: vec![variant("KeyValue", Some("kv"))],
            lookup: "key-value",
            expected: None,
        },
        Case {
            name: "an unrenamed variant has nothing to reconcile and is absent",
            rename_all: None,
            variants: vec![variant("Plain", None)],
            lookup: "Plain",
            expected: None,
        },
        Case {
            name: "a rename_all that is a no-op for this identifier is absent",
            rename_all: Some("PascalCase"),
            variants: vec![variant("Plain", None)],
            lookup: "Plain",
            expected: None,
        },
        Case {
            name: "two variants renamed onto one wire value are ambiguous and dropped",
            rename_all: None,
            variants: vec![variant("First", Some("shared")), variant("Second", Some("shared"))],
            lookup: "shared",
            expected: None,
        },
        Case {
            name: "a wire value that is another variant's identifier is valid on both surfaces and dropped",
            rename_all: None,
            variants: vec![variant("Alpha", Some("Beta")), variant("Beta", None)],
            lookup: "Beta",
            expected: None,
        },
    ];

    for case in cases {
        let enums = vec![wire_variant_enum(case.rename_all, case.variants)];
        let map = build_ir_enum_map(&[], &enums);
        let got = map
            .enum_wire_variants
            .get("Kind")
            .and_then(|by_wire| by_wire.get(case.lookup))
            .map(String::as_str);
        assert_eq!(got, case.expected, "case '{}'", case.name);
    }
}

/// `variant_payload_tuple` must answer whether serde actually FLATTENS a variant's payload
/// beside the discriminator (`serde_enum_repr::serde_flattens_newtype_payload`), not merely
/// whether the variant is tuple-shaped (`EnumVariant::is_tuple`). An internally tagged newtype
/// variant flattens; an adjacently tagged, untagged, or externally tagged newtype variant does
/// not, even though all four are `Variant(Payload)`-shaped on the Rust side. Getting this wrong
/// once already turned every tagged-enum payload assertion in a consumer's Ruby suite into a
/// `KeyError` (alef 0.85.11 -- see `types::IrEnumMap::variant_payload_tuple`'s doc comment).
#[test]
fn variant_payload_tuple_is_true_only_for_the_flattening_representation() {
    fn newtype_variant(name: &str) -> EnumVariant {
        EnumVariant {
            name: name.to_string(),
            fields: vec![field("_0", TypeRef::Named("Payload".to_string()))],
            is_tuple: true,
            ..EnumVariant::default()
        }
    }

    let internal = EnumDef {
        name: "Internal".to_string(),
        serde_tag: Some("type".to_string()),
        variants: vec![newtype_variant("Variant")],
        ..EnumDef::default()
    };
    let adjacent = EnumDef {
        name: "Adjacent".to_string(),
        serde_tag: Some("type".to_string()),
        serde_content: Some("payload".to_string()),
        variants: vec![newtype_variant("Variant")],
        ..EnumDef::default()
    };
    let untagged = EnumDef {
        name: "Untagged".to_string(),
        serde_untagged: true,
        variants: vec![newtype_variant("Variant")],
        ..EnumDef::default()
    };
    let external = EnumDef {
        name: "External".to_string(),
        variants: vec![newtype_variant("Variant")],
        ..EnumDef::default()
    };

    let map = build_ir_enum_map(&[], &[internal, adjacent, untagged, external]);

    assert!(
        map.variant_payload_tuple
            .get("Internal")
            .is_some_and(|variants| variants.contains("Variant")),
        "internal tagging flattens the newtype payload beside the tag: {:?}",
        map.variant_payload_tuple
    );
    for name in ["Adjacent", "Untagged", "External"] {
        assert!(
            !map.variant_payload_tuple
                .get(name)
                .is_some_and(|variants| variants.contains("Variant")),
            "{name} tagging must NOT be recorded as flattened: {:?}",
            map.variant_payload_tuple
        );
    }

    // The single-payload-type resolution itself is representation-agnostic and must still work
    // for all four, since a caller may still need to walk into the payload's own fields under
    // the non-flattened hop (`content` key, or the bare untagged payload).
    for name in ["Internal", "Adjacent", "Untagged", "External"] {
        assert_eq!(
            map.variant_payload_types.get(name).and_then(|v| v.get("Variant")),
            Some(&("_0".to_string(), "Payload".to_string())),
            "{name} must still resolve the payload type regardless of flattening"
        );
    }
}
