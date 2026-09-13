//! The serde wire-name surface: JSON/serialization names, never host-language identifiers.
//!
//! `serde_rename` and `serde_rename_all` define wire names only. Nothing here may be used to
//! spell a public host identifier — that is [`super::host`]'s job. ~keep

use super::case::{pascal_to_screaming_snake, pascal_to_snake};
use crate::core::ir::{FieldDef, TypeRef};
use heck::{ToKebabCase, ToLowerCamelCase, ToPascalCase};

/// Apply a serde `rename_all` strategy to a Rust identifier.
pub fn apply_serde_rename_all(name: &str, rename_all: Option<&str>) -> String {
    match rename_all {
        Some("lowercase") => name.to_ascii_lowercase(),
        Some("UPPERCASE") => name.to_ascii_uppercase(),
        Some("PascalCase") => name.to_pascal_case(),
        Some("camelCase") => name.to_lower_camel_case(),
        Some("snake_case") => pascal_to_snake(name),
        Some("SCREAMING_SNAKE_CASE") => pascal_to_screaming_snake(name),
        Some("kebab-case") => pascal_to_snake(name).to_kebab_case(),
        Some("SCREAMING-KEBAB-CASE") => pascal_to_snake(name).to_kebab_case().to_ascii_uppercase(),
        Some(_) | None => name.to_string(),
    }
}

/// Resolve a serde wire name, with explicit `serde(rename)` taking precedence.
pub fn serde_wire_name(rust_name: &str, serde_rename: Option<&str>, rename_all: Option<&str>) -> String {
    serde_rename
        .map(str::to_string)
        .unwrap_or_else(|| apply_serde_rename_all(rust_name, rename_all))
}

/// Resolve a wire field name from field metadata.
pub fn wire_field_name(field_name: &str, serde_rename: Option<&str>, rename_all: Option<&str>) -> String {
    serde_wire_name(field_name, serde_rename, rename_all)
}

/// Resolve a wire field name for the napi/wasm JSON-passthrough emitters, where the product
/// decision is that JavaScript-facing JSON is camelCase all the way down (including nested
/// payloads), independent of whatever `rename_all` the Rust container declares for its own
/// serde wire format. An explicit `#[serde(rename = "...")]` is author intent and still wins
/// verbatim -- only a name derived mechanically from the Rust field identifier gets recased.
///
/// This is a deliberate opt-in, not a change to [`wire_field_name`]'s behaviour: that function
/// is shared by every other backend's wire-name resolution and must keep producing serde's own
/// (typically snake_case) names for them. Only the napi `WireTypes` emitter and the wasm
/// `TsMapContext` emitter -- the two places that declare the `.d.ts` shape of a JSON-passthrough
/// value for a JS/TS consumer -- may call this. ~keep
pub fn wire_field_name_camel(field_name: &str, serde_rename: Option<&str>) -> String {
    serde_rename
        .map(str::to_string)
        .unwrap_or_else(|| field_name.to_lower_camel_case())
}

/// Resolve a wire enum variant value from variant metadata.
pub fn wire_variant_value(variant_name: &str, serde_rename: Option<&str>, rename_all: Option<&str>) -> String {
    serde_wire_name(variant_name, serde_rename, rename_all)
}

/// True when a `Duration`-typed field's wire shape is the object serde's derive produces
/// (`{"secs":u64,"nanos":u32}`) rather than the shape a hand-written codec writes.
///
/// A field carrying `#[serde(with = "...")]` (or `serialize_with`) does not get its wire shape
/// from serde's derive at all — the codec decides it, and the common `duration_ms` convention
/// writes a bare millisecond integer instead of the derive object. Backends that special-case
/// `TypeRef::Duration`'s derive shape (Go's `DurationMillis` wrapper, C#'s
/// `DurationMillisJsonConverter`, Java's `DurationMillisSerializer`/`Deserializer`) must consult
/// this single predicate before imposing the map shape on a `Duration` field — asserting it
/// unconditionally breaks every field serialized through such a codec with
/// `invalid type: map, expected u64`. See `FieldDef::serde_with`.
pub fn field_uses_duration_map_wire(field: &FieldDef) -> bool {
    matches!(field.ty, TypeRef::Duration) && field.serde_with.is_none()
}
