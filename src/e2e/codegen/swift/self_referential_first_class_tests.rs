//! Regression coverage pinning agreement between the Swift *binding* emitter's first-class/
//! opaque classification (`backends::swift::gen_bindings::dto::compute_first_class_dto_names`)
//! and the Swift *e2e* generator's own copy of that classification
//! (`values::build_swift_first_class_map`) for a self-referential DTO — a struct that holds
//! itself only through `Vec<Self>` (or `Option<Vec<Self>>`), the recursive-tree shape
//! `DocumentNode.children` is.
//!
//! `compute_first_class_dto_names`'s fixed-point loop accepts a field via
//! `first_class_field_supported(&field.ty, &known) || is_self_reference_through_indirection(&field.ty, &ty.name)`
//! — the second arm exists because a bare fixed point can never bootstrap a self-reference: a
//! field of `Vec<Self>` can never satisfy `first_class_field_supported` from an EMPTY `known`
//! set, since `Self` cannot be in `known` before the loop has decided `Self` is first-class, and
//! `Self` can never be decided first-class before this one field is satisfied. Without the OR
//! arm, no self-referential type could ever converge to first-class in EITHER classifier.
//!
//! `build_swift_first_class_map`'s own loop was a straight port of the fixed point without that
//! OR arm, so it never classifies a self-referential type as first-class, while the binding
//! emitter — walking the identical IR — does. That is the exact defect shape a peer report
//! described (binding emits stored properties, e2e still emits `()` method-call syntax against
//! the same type) even though the general per-field predicate the two classifiers share
//! (`swift_first_class_field_supported`, delegating to `dto::first_class_field_supported`) has
//! been unified since before this repo's Swift first-class classification existed at all.
//!
//! This test builds one recursive IR type and asserts membership in BOTH classifiers' first-class
//! sets so it fails if either classifier changes alone.
use crate::backends::swift::gen_bindings::dto::compute_first_class_dto_names;
use crate::core::ir::{ApiSurface, FieldDef, TypeDef, TypeRef};
use crate::e2e::config::{CallConfig, E2eConfig};
use std::collections::HashSet;

fn recursive_tree_node() -> TypeDef {
    TypeDef {
        name: "TreeNode".to_string(),
        has_serde: true,
        fields: vec![
            FieldDef {
                name: "label".to_string(),
                ty: TypeRef::String,
                ..FieldDef::default()
            },
            FieldDef {
                name: "children".to_string(),
                ty: TypeRef::Vec(Box::new(TypeRef::Named("TreeNode".to_string()))),
                ..FieldDef::default()
            },
        ],
        ..TypeDef::default()
    }
}

/// The binding emitter's own classifier must treat a `Vec<Self>`-recursive DTO as first-class —
/// this is the fixture's control, proving `is_self_reference_through_indirection` fires at all
/// before comparing the e2e side against it.
#[test]
fn the_binding_classifier_accepts_a_vec_self_recursive_dto() {
    let type_def = recursive_tree_node();
    let api = ApiSurface {
        types: vec![type_def],
        ..ApiSurface::default()
    };
    let known = compute_first_class_dto_names(&api, &HashSet::new());
    assert!(
        known.contains("TreeNode"),
        "binding classifier must accept a Vec<Self>-recursive DTO as first-class, got: {known:?}"
    );
}

/// The e2e classifier must agree with the binding classifier for the exact same IR — a
/// self-referential DTO the binding emits with stored properties must not still be walked with
/// opaque method-call syntax by the generated e2e suite.
#[test]
fn the_e2e_classifier_agrees_with_the_binding_classifier_on_a_vec_self_recursive_dto() {
    let type_def = recursive_tree_node();
    let type_defs = vec![type_def];
    let e2e_config = E2eConfig::default();
    let call_config = CallConfig::default();
    let map = super::values::build_swift_first_class_map(&type_defs, &[], &e2e_config, &call_config);
    assert!(
        map.is_first_class(Some("TreeNode")),
        "e2e classifier disagreed with the binding classifier: TreeNode is first-class on the \
         binding side (stored properties) but the e2e generator still classified it opaque \
         (method-call syntax), which is exactly the compile-break shape a two-predicate drift \
         produces"
    );
}
