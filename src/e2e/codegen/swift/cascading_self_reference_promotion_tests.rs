//! Regression coverage for the exact IR shape a peer report traced by reading alef's source: a
//! self-referential tree node reached through an `Option<Named>` field on a container that is
//! NOT itself self-referential.
//!
//! ```text
//! pub struct DataNode {
//!     pub kind: DataNodeKind,
//!     pub children: Vec<DataNode>,   // self-reference through Vec -- the bootstrap case
//! }
//! pub struct ProcessResult {
//!     pub data: Option<DataNode>,    // ordinary Optional<Named>, NOT self-referential itself
//! }
//! ```
//!
//! Their hypothesis: `DataNode` becomes first-class only via the self-reference bootstrap
//! (`is_self_reference_through_indirection`, see `dto.rs`'s `~keep` comment on why it is
//! `pub(crate)`); that promotion CASCADES `ProcessResult` into first-class too, because ordinary
//! `first_class_field_supported` accepts `Option<Named(DataNode))` once `DataNode` itself is
//! known. If the e2e fixed point does not apply the identical bootstrap, `DataNode` never enters
//! its `known_dto_names`, so the cascade never reaches `ProcessResult` either, and
//! `SwiftFirstClassMap` is missing BOTH types even though the binding emitter promoted both.
//!
//! `self_referential_first_class_tests.rs` already pins the bootstrap itself for the minimal
//! shape (a type self-referencing through `Vec<Self>` directly). This file pins the SEPARATE
//! fact their report specifically asked to be checked honestly: that the promotion actually
//! propagates one hop further, to a container that only reaches the self-referential type through
//! a plain `Option<Named>` field -- proving this is not just the bootstrap firing on `DataNode`
//! in isolation, but the fixed point actually cascading the way `compute_first_class_dto_names`
//! does.
use crate::backends::swift::gen_bindings::dto::compute_first_class_dto_names;
use crate::core::ir::{ApiSurface, FieldDef, TypeDef, TypeRef};
use crate::e2e::config::{CallConfig, E2eConfig};
use std::collections::HashSet;

fn data_node_and_process_result() -> Vec<TypeDef> {
    vec![
        TypeDef {
            name: "DataNode".to_string(),
            has_serde: true,
            fields: vec![
                FieldDef {
                    name: "kind".to_string(),
                    ty: TypeRef::String,
                    ..FieldDef::default()
                },
                FieldDef {
                    name: "children".to_string(),
                    ty: TypeRef::Vec(Box::new(TypeRef::Named("DataNode".to_string()))),
                    ..FieldDef::default()
                },
            ],
            ..TypeDef::default()
        },
        TypeDef {
            name: "ProcessResult".to_string(),
            has_serde: true,
            fields: vec![FieldDef {
                name: "data".to_string(),
                ty: TypeRef::Optional(Box::new(TypeRef::Named("DataNode".to_string()))),
                ..FieldDef::default()
            }],
            ..TypeDef::default()
        },
    ]
}

/// Control: the binding emitter's own fixed point must cascade the self-reference bootstrap on
/// `DataNode` one hop further into `ProcessResult`, which only reaches `DataNode` through a
/// plain `Option<Named>` field. Proves the falsifiable claim before checking the e2e side.
#[test]
fn the_binding_classifier_cascades_the_bootstrap_through_an_optional_named_field() {
    let api = ApiSurface {
        types: data_node_and_process_result(),
        ..ApiSurface::default()
    };
    let known = compute_first_class_dto_names(&api, &HashSet::new());
    assert!(
        known.contains("DataNode"),
        "control: DataNode must bootstrap first, got: {known:?}"
    );
    assert!(
        known.contains("ProcessResult"),
        "control: the promotion must cascade to ProcessResult through Option<DataNode>, got: {known:?}"
    );
}

/// The e2e classifier must agree with the binding classifier on BOTH types for the identical
/// IR -- this is the falsifier the report asked to be checked honestly. If `ProcessResult` were
/// present in `SwiftFirstClassMap` even without the self-reference fix, that would refute the
/// two-fixed-points hypothesis; it is not (see the pre-fix note in
/// `self_referential_first_class_tests.rs`), confirming the cascade was broken and is now fixed
/// by the same `is_self_reference_through_indirection` delegation.
#[test]
fn the_e2e_classifier_agrees_on_both_the_bootstrapped_type_and_its_cascaded_parent() {
    let type_defs = data_node_and_process_result();
    let e2e_config = E2eConfig::default();
    let call_config = CallConfig::default();
    let map = super::values::build_swift_first_class_map(&type_defs, &[], &e2e_config, &call_config);
    assert!(
        map.is_first_class(Some("DataNode")),
        "e2e classifier must promote DataNode via the self-reference bootstrap"
    );
    assert!(
        map.is_first_class(Some("ProcessResult")),
        "e2e classifier must cascade the promotion to ProcessResult through Option<DataNode> -- \
         a miss here reproduces the reported whole-result-flips-to-opaque symptom, since \
         `SwiftFirstClassMap::is_first_class(None)` and `is_first_class(Some(\"ProcessResult\"))` \
         degrade identically to method-call syntax when ProcessResult is absent from the map"
    );
}
