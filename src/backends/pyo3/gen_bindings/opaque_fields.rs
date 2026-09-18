use crate::codegen::generators::structs::field_references_opaque_type;
use crate::core::ir::ApiSurface;
use ahash::AHashSet;

/// Propagate non-serializable fields through containing records. Data-enum wrappers have
/// their own serde implementations, so they must not seed this closure. ~keep
pub(super) fn extend_nonserializable_records(
    api: &ApiSurface,
    opaque_names: &mut Vec<String>,
    serializable_names: &[String],
) {
    let mut known_names: AHashSet<String> = opaque_names.iter().cloned().collect();
    let mut nonserializable_names: Vec<String> = opaque_names
        .iter()
        .filter(|name| !serializable_names.contains(name))
        .cloned()
        .collect();
    let mut changed = true;
    while changed {
        changed = false;
        for typ in api.types.iter().filter(|typ| !typ.is_opaque) {
            if known_names.contains(&typ.name) {
                continue;
            }
            if typ
                .fields
                .iter()
                .any(|field| field_references_opaque_type(&field.ty, &nonserializable_names))
            {
                opaque_names.push(typ.name.clone());
                nonserializable_names.push(typ.name.clone());
                known_names.insert(typ.name.clone());
                changed = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::extend_nonserializable_records;
    use crate::core::ir::{ApiSurface, FieldDef, TypeDef, TypeRef};

    fn containing_records(leaf: &str) -> ApiSurface {
        let named = |name: &str| TypeRef::Named(name.into());
        let fields = [
            ("Batch", TypeRef::Vec(Box::new(named("Prepared")))),
            ("Prepared", TypeRef::Optional(Box::new(named("Request")))),
            (
                "Request",
                TypeRef::Map(Box::new(TypeRef::String), Box::new(named(leaf))),
            ),
        ];
        ApiSurface {
            types: fields
                .into_iter()
                .map(|(name, ty)| TypeDef {
                    name: name.into(),
                    fields: vec![FieldDef {
                        name: "value".into(),
                        ty,
                        ..Default::default()
                    }],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn serializable_data_enums_do_not_taint_containing_records() {
        let mut names = vec!["Policy".into(), "Handle".into()];
        extend_nonserializable_records(&containing_records("Policy"), &mut names, &["Policy".into()]);
        assert_eq!(names, ["Policy", "Handle"]);
    }

    #[test]
    fn opaque_handles_and_bridge_aliases_still_propagate_to_a_fixed_point() {
        for leaf in ["Handle", "VisitorAlias"] {
            let mut names = vec!["Policy".into(), leaf.into()];
            extend_nonserializable_records(&containing_records(leaf), &mut names, &["Policy".into()]);
            assert_eq!(names, ["Policy", leaf, "Request", "Prepared", "Batch"]);
        }
    }
}
