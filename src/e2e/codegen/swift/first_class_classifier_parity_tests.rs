//! Structural parity gate between the Swift *binding* emitter's first-class/opaque classifier
//! (`backends::swift::gen_bindings::dto::compute_first_class_dto_names`, which feeds
//! `first_class_types` in `gen_bindings::mod.rs`) and the Swift *e2e* generator's own copy
//! (`e2e::codegen::swift::values::build_swift_first_class_map`'s `first_class_types`).
//!
//! A peer report proved these two can silently diverge: `alef e2e generate` regenerated 34 files
//! and exited 0, byte-identical, across a binding regen that promoted three types (`DataNode`,
//! `StructureItem`, `ProcessResult`) from opaque to first-class. Nothing failed loudly because
//! nothing ever asked whether the two fixed points still agreed -- the e2e side's
//! `known_dto_names` loop (`values.rs`) lacked the `is_self_reference_through_indirection`
//! bootstrap the binding side's loop (`dto.rs::compute_first_class_dto_names`) has always had, so
//! a `struct Node { children: Vec<Node> }` shape (and anything that CASCADES from one, like a
//! container reaching it only through `Option<Named>`) silently classified opposite ways on the
//! two sides.
//!
//! The per-field fix (`values.rs` now calls
//! `dto::is_self_reference_through_indirection` directly, the same `pub(crate)` function the
//! binding emitter calls, rather than a second copy of the rule) closes the specific gap. This
//! file is the structural gate the report asked for on top of that: it runs BOTH classifiers over
//! several IR shapes -- plain, opaque, self-referential via `Vec`, self-referential via `Map`,
//! a cascading container, a mutual (two-hop) self-reference, and the one shape that must stay
//! opaque on both sides (a bare, non-indirected `Optional<Named(self)>`, which Swift's value-type
//! `Optional` cannot represent recursively) -- and asserts the two `first_class_types` SETS are
//! identical for each. Unlike `self_referential_first_class_tests.rs` and
//! `cascading_self_reference_promotion_tests.rs`, which each assert one type's membership by
//! name, this asserts full-set equality: any future asymmetry between the two fixed points, in
//! either direction and on any shape, fails this test even if no one thinks to add a case for it
//! by name.
use crate::backends::swift::gen_bindings::dto::compute_first_class_dto_names;
use crate::core::ir::{ApiSurface, FieldDef, TypeDef, TypeRef};
use crate::e2e::config::{CallConfig, E2eConfig};
use std::collections::HashSet;

fn dto(name: &str, fields: Vec<FieldDef>) -> TypeDef {
    TypeDef {
        name: name.to_string(),
        has_serde: true,
        fields,
        ..TypeDef::default()
    }
}

fn field(name: &str, ty: TypeRef) -> FieldDef {
    FieldDef {
        name: name.to_string(),
        ty,
        ..FieldDef::default()
    }
}

/// Both classifiers' `first_class_types` set, computed over the identical `type_defs`, with no
/// enums and no `exclude_types` on either side (both left empty is a superset the e2e side always
/// accepted -- see `build_swift_first_class_map`'s own doc comment on `exclude_types`).
fn both_classifiers(type_defs: &[TypeDef]) -> (HashSet<String>, HashSet<String>) {
    let api = ApiSurface {
        types: type_defs.to_vec(),
        ..ApiSurface::default()
    };
    let binding_known = compute_first_class_dto_names(&api, &HashSet::new());
    // `first_class_types` on the binding side is `known_dto_names` restricted to type (not enum)
    // names, matching what the e2e side reports -- see the two `mod.rs`/`values.rs` filters cited
    // in this file's module doc.
    let binding_first_class: HashSet<String> = type_defs
        .iter()
        .filter(|t| binding_known.contains(&t.name))
        .map(|t| t.name.clone())
        .collect();

    let e2e_config = E2eConfig::default();
    let call_config = CallConfig::default();
    let map = super::values::build_swift_first_class_map(type_defs, &[], &e2e_config, &call_config);
    (binding_first_class, map.first_class_types)
}

fn assert_parity(case: &str, type_defs: &[TypeDef]) {
    let (binding, e2e) = both_classifiers(type_defs);
    assert_eq!(
        binding,
        e2e,
        "{case}: binding and e2e classifiers disagree -- binding only: {:?}, e2e only: {:?}",
        binding.difference(&e2e).collect::<Vec<_>>(),
        e2e.difference(&binding).collect::<Vec<_>>()
    );
}

#[test]
fn plain_scalar_struct_agrees() {
    let type_defs = vec![dto("Simple", vec![field("name", TypeRef::String)])];
    assert_parity("plain scalar struct", &type_defs);
}

#[test]
fn a_struct_with_an_unsupported_field_stays_opaque_on_both_sides() {
    let type_defs = vec![dto("Opaque", vec![field("payload", TypeRef::Bytes)])];
    assert_parity("Bytes-field struct", &type_defs);
}

#[test]
fn vec_self_reference_agrees() {
    let type_defs = vec![dto(
        "DataNode",
        vec![
            field("kind", TypeRef::String),
            field(
                "children",
                TypeRef::Vec(Box::new(TypeRef::Named("DataNode".to_string()))),
            ),
        ],
    )];
    assert_parity("Vec<Self> self-reference", &type_defs);
}

#[test]
fn map_value_self_reference_agrees() {
    let type_defs = vec![dto(
        "IndexNode",
        vec![field(
            "byKey",
            TypeRef::Map(
                Box::new(TypeRef::String),
                Box::new(TypeRef::Named("IndexNode".to_string())),
            ),
        )],
    )];
    assert_parity("Map<String, Self> self-reference", &type_defs);
}

/// The exact reported shape: two independently self-referential types (`DataNode`,
/// `StructureItem`), plus a container (`ProcessResult`) that is not itself self-referential but
/// reaches both only through ordinary `Vec<Named>`/`Option<Named>` fields, and so only becomes
/// classifiable once the fixed point has already promoted its members.
#[test]
fn a_cascading_container_over_two_self_referential_members_agrees() {
    let type_defs = vec![
        dto(
            "DataNode",
            vec![
                field("kind", TypeRef::String),
                field(
                    "children",
                    TypeRef::Vec(Box::new(TypeRef::Named("DataNode".to_string()))),
                ),
            ],
        ),
        dto(
            "StructureItem",
            vec![
                field("label", TypeRef::String),
                field(
                    "children",
                    TypeRef::Vec(Box::new(TypeRef::Named("StructureItem".to_string()))),
                ),
            ],
        ),
        dto(
            "ProcessResult",
            vec![
                field(
                    "structure",
                    TypeRef::Vec(Box::new(TypeRef::Named("StructureItem".to_string()))),
                ),
                field(
                    "data",
                    TypeRef::Optional(Box::new(TypeRef::Named("DataNode".to_string()))),
                ),
            ],
        ),
    ];
    assert_parity("cascading container over two self-referential members", &type_defs);
}

/// A two-hop MUTUAL indirected self-reference (`A` reaches itself only via `B`, and vice versa),
/// not a direct one -- exercises the fixed point actually iterating to convergence rather than a
/// single bootstrap pass.
#[test]
fn mutual_indirected_self_reference_agrees() {
    let type_defs = vec![
        dto(
            "Branch",
            vec![field(
                "leaves",
                TypeRef::Vec(Box::new(TypeRef::Named("Leaf".to_string()))),
            )],
        ),
        dto(
            "Leaf",
            vec![field(
                "branches",
                TypeRef::Vec(Box::new(TypeRef::Named("Branch".to_string()))),
            )],
        ),
    ];
    assert_parity("mutual Vec-indirected self-reference", &type_defs);
}

/// The edge `is_self_reference_through_indirection` deliberately declines: a BARE
/// `Optional<Named(self)>` with no `Vec`/`Map` indirection. Swift's value-type `Optional` cannot
/// represent `struct Foo { var next: Foo? }`, so this must stay opaque on both sides -- a parity
/// test that only ever checks the accepted cases would not catch a future change that widened one
/// side's acceptance past what Swift can actually compile.
#[test]
fn bare_optional_self_reference_stays_opaque_on_both_sides() {
    let type_defs = vec![dto(
        "LinkedNode",
        vec![field(
            "next",
            TypeRef::Optional(Box::new(TypeRef::Named("LinkedNode".to_string()))),
        )],
    )];
    assert_parity("bare Optional<Self> (not indirected)", &type_defs);
}

/// The second proven trigger: a struct with NO self-reference at all, whose only unusual field
/// is a `Map<String, String>` (`HashMap<Box<str>, Box<str>>` at the Rust source, surfacing as
/// `[String: String]`). This shape was reported by a downstream consumer as a second, independent
/// promotion trigger. A `#[alef(skip)]`
/// field (`content: Vec<u8>`) is included to match the real shape and to prove
/// `binding_excluded` filtering agrees on both sides too.
#[test]
fn a_map_field_struct_with_no_self_reference_agrees() {
    let type_defs = vec![dto(
        "DownloadedDocument",
        vec![
            field("url", TypeRef::String),
            field("mimeType", TypeRef::String),
            FieldDef {
                name: "content".to_string(),
                ty: TypeRef::Vec(Box::new(TypeRef::Primitive(crate::core::ir::PrimitiveType::U8))),
                binding_excluded: true,
                ..FieldDef::default()
            },
            field("size", TypeRef::Primitive(crate::core::ir::PrimitiveType::Usize)),
            field("filename", TypeRef::Optional(Box::new(TypeRef::String))),
            field("contentHash", TypeRef::String),
            field(
                "headers",
                TypeRef::Map(Box::new(TypeRef::String), Box::new(TypeRef::String)),
            ),
            field("truncated", TypeRef::Primitive(crate::core::ir::PrimitiveType::Bool)),
            field("contentPath", TypeRef::Optional(Box::new(TypeRef::String))),
        ],
    )];
    assert_parity(
        "map-field struct, no self-reference (DownloadedDocument shape)",
        &type_defs,
    );
    // Parity alone would also hold if BOTH classifiers rejected this type, which is precisely how
    // this gate could rot into a vacuous pass: the reported defect is that the binding side
    // promotes it, so a run where neither side does would agree for the wrong reason and still
    // leave the consumer broken. Pin the membership too, so the parity above is known to be
    // parity on a type that is actually first-class. ~keep
    let (binding, _) = both_classifiers(&type_defs);
    assert!(
        binding.contains("DownloadedDocument"),
        "the map-field shape must be first-class on the binding side, else the parity assertion \
         above proves nothing; got: {binding:?}"
    );
}
