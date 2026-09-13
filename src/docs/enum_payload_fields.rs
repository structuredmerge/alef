//! Renders the "Fields: ..." annotation attached to an enum variant's doc row so it matches
//! the variant's actual wire/binding shape rather than alef's internal IR field names.
//!
//! `Excel(ExcelMetadata)` is encoded in the IR as a tuple variant with one synthesized field
//! named `_0` -- a name the extractor invented, never written by a user and present on no wire
//! or binding surface. Rendering it verbatim advertises a key nothing emits. This module is the
//! single place both the shared, language-neutral pages (`types.md`, `configuration.md`) and the
//! per-language pages (`api-{lang}.md`) resolve a variant's real field names from, so they cannot
//! independently drift on which shape is correct for a given serde representation.
//!
//! Uses [`crate::codegen::serde_enum_repr`] (already the single source of truth for serde's four
//! enum representations) rather than re-deriving the classification here.

use crate::codegen::serde_enum_repr::{SerdeEnumRepr, serde_enum_repr, serde_flattens_newtype_payload};
use crate::codegen::shared::binding_fields;
use crate::core::ir::{ApiSurface, EnumDef, EnumVariant, FieldDef, TypeRef};

/// Resolve the field descriptions and the label to show for one enum variant.
///
/// `field_name_for` renders a single field's display name and `type_desc_for` renders a single
/// field's type; both are supplied by the caller because the two call sites use different
/// renderers (per-language `docs::naming::field_name` + `doc_type`, vs. the shared pages'
/// Rust-canonical field name and `format_type_ref_rust`).
///
/// Three shapes, matching [`SerdeEnumRepr`]:
/// - Internal tagging flattens a single-field tuple variant's payload into the tag-bearing
///   object (see [`serde_flattens_newtype_payload`]): render the payload TYPE's own fields, not
///   the synthesized `_0`.
/// - Adjacent tagging keeps the payload under its own `content` key: render that key name in
///   place of `_0`.
/// - External and untagged representations keep a real key for the payload already (the variant
///   name, or the bare payload respectively), so a tuple variant's positional field name is left
///   as-is here.
pub(crate) fn variant_field_descriptions<FName, FType>(
    en: &EnumDef,
    variant: &EnumVariant,
    variant_fields: &[&FieldDef],
    api: &ApiSurface,
    field_name_for: FName,
    type_desc_for: FType,
) -> (Vec<String>, &'static str)
where
    FName: Fn(&str) -> String,
    FType: Fn(&FieldDef) -> String,
{
    let single_tuple_payload = variant.is_tuple && variant.fields.len() == 1;

    if single_tuple_payload && serde_flattens_newtype_payload(en, variant, &api.types) {
        let payload_field = &variant.fields[0];
        let descs = flattened_payload_field_descs(payload_field, api, &field_name_for, &type_desc_for);
        return (descs, "Fields (flattened into the tagged object)");
    }

    if single_tuple_payload && let SerdeEnumRepr::Adjacent { content, .. } = serde_enum_repr(en) {
        let payload_field = &variant.fields[0];
        return (
            vec![format!("`{content}`: `{}`", type_desc_for(payload_field))],
            "Fields",
        );
    }

    let descs = variant_fields
        .iter()
        .map(|f| format!("`{}`: `{}`", field_name_for(&f.name), type_desc_for(f)))
        .collect();
    (descs, "Fields")
}

/// The payload type's own fields, for a variant whose payload serde flattens onto the wire.
/// Falls back to the raw positional field if the payload type is not a `Named` reference to a
/// struct this crate's IR resolved (e.g. an opaque or externally defined type).
fn flattened_payload_field_descs<FName, FType>(
    payload_field: &FieldDef,
    api: &ApiSurface,
    field_name_for: &FName,
    type_desc_for: &FType,
) -> Vec<String>
where
    FName: Fn(&str) -> String,
    FType: Fn(&FieldDef) -> String,
{
    let payload_type_name = match &payload_field.ty {
        TypeRef::Named(name) => Some(name.as_str()),
        _ => None,
    };
    let payload_type = payload_type_name.and_then(|name| api.types.iter().find(|t| t.name == name));
    match payload_type {
        Some(type_def) => binding_fields(&type_def.fields)
            .map(|f| format!("`{}`: `{}`", field_name_for(&f.name), type_desc_for(f)))
            .collect(),
        None => vec![format!(
            "`{}`: `{}`",
            field_name_for(&payload_field.name),
            type_desc_for(payload_field)
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ir::{PrimitiveType, TypeDef};

    fn tuple_variant(name: &str, field_name: &str, field_ty: TypeRef) -> EnumVariant {
        EnumVariant {
            name: name.to_string(),
            is_tuple: true,
            fields: vec![FieldDef {
                name: field_name.to_string(),
                ty: field_ty,
                ..FieldDef::default()
            }],
            ..EnumVariant::default()
        }
    }

    fn struct_variant(name: &str, fields: &[(&str, TypeRef)]) -> EnumVariant {
        EnumVariant {
            name: name.to_string(),
            is_tuple: false,
            fields: fields
                .iter()
                .map(|(fname, ty)| FieldDef {
                    name: (*fname).to_string(),
                    ty: ty.clone(),
                    ..FieldDef::default()
                })
                .collect(),
            ..EnumVariant::default()
        }
    }

    fn identity_name(name: &str) -> String {
        name.to_string()
    }

    fn primitive_type_desc(field: &FieldDef) -> String {
        match &field.ty {
            TypeRef::Primitive(PrimitiveType::U32) => "u32".to_string(),
            TypeRef::Named(name) => name.clone(),
            _ => "unknown".to_string(),
        }
    }

    #[test]
    fn should_render_flattened_payload_fields_instead_of_the_positional_name() {
        let excel_metadata = TypeDef {
            name: "ExcelMetadata".to_string(),
            fields: vec![FieldDef {
                name: "sheet_count".to_string(),
                ty: TypeRef::Primitive(PrimitiveType::U32),
                ..FieldDef::default()
            }],
            ..TypeDef::default()
        };
        let api = ApiSurface {
            types: vec![excel_metadata],
            ..ApiSurface::default()
        };
        let en = EnumDef {
            name: "FormatMetadata".to_string(),
            serde_tag: Some("format_type".to_string()),
            variants: vec![tuple_variant(
                "Excel",
                "_0",
                TypeRef::Named("ExcelMetadata".to_string()),
            )],
            ..EnumDef::default()
        };
        let variant = &en.variants[0];
        let variant_fields: Vec<_> = variant.fields.iter().collect();

        let (descs, label) =
            variant_field_descriptions(&en, variant, &variant_fields, &api, identity_name, primitive_type_desc);

        assert_eq!(descs, vec!["`sheet_count`: `u32`"]);
        assert_eq!(label, "Fields (flattened into the tagged object)");
    }

    #[test]
    fn should_render_the_content_key_for_an_adjacently_tagged_newtype_variant() {
        let api = ApiSurface::default();
        let en = EnumDef {
            name: "FormatMetadata".to_string(),
            serde_tag: Some("format_type".to_string()),
            serde_content: Some("payload".to_string()),
            variants: vec![tuple_variant(
                "Excel",
                "_0",
                TypeRef::Named("ExcelMetadata".to_string()),
            )],
            ..EnumDef::default()
        };
        let variant = &en.variants[0];
        let variant_fields: Vec<_> = variant.fields.iter().collect();

        let (descs, label) =
            variant_field_descriptions(&en, variant, &variant_fields, &api, identity_name, primitive_type_desc);

        assert_eq!(descs, vec!["`payload`: `ExcelMetadata`"]);
        assert_eq!(label, "Fields");
    }

    /// Negative control: an externally tagged newtype variant is a representation serde does
    /// NOT flatten, so its current positional rendering must be left untouched.
    #[test]
    fn should_keep_the_positional_field_name_for_a_representation_serde_does_not_flatten() {
        let api = ApiSurface::default();
        let en = EnumDef {
            name: "FormatMetadata".to_string(),
            variants: vec![tuple_variant(
                "Excel",
                "_0",
                TypeRef::Named("ExcelMetadata".to_string()),
            )],
            ..EnumDef::default()
        };
        let variant = &en.variants[0];
        let variant_fields: Vec<_> = variant.fields.iter().collect();

        let (descs, label) =
            variant_field_descriptions(&en, variant, &variant_fields, &api, identity_name, primitive_type_desc);

        assert_eq!(descs, vec!["`_0`: `ExcelMetadata`"]);
        assert_eq!(label, "Fields");
    }

    /// A struct variant's fields are already real, wire-accurate names under every
    /// representation -- flattening logic must never engage for them.
    #[test]
    fn should_keep_struct_variant_field_names_for_internal_tagging() {
        let api = ApiSurface::default();
        let en = EnumDef {
            name: "FormatMetadata".to_string(),
            serde_tag: Some("format_type".to_string()),
            variants: vec![struct_variant(
                "Pdf",
                &[("page_count", TypeRef::Primitive(PrimitiveType::U32))],
            )],
            ..EnumDef::default()
        };
        let variant = &en.variants[0];
        let variant_fields: Vec<_> = variant.fields.iter().collect();

        let (descs, label) =
            variant_field_descriptions(&en, variant, &variant_fields, &api, identity_name, primitive_type_desc);

        assert_eq!(descs, vec!["`page_count`: `u32`"]);
        assert_eq!(label, "Fields");
    }
}
