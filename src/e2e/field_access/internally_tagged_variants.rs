//! Which fixture-path segments a JSON-navigating e2e backend must SKIP because they name a
//! serde *variant* rather than a wire key.
//!
//! An internally tagged enum (`#[serde(tag = "...")]` with no `content`) has a FLAT wire form:
//! the selected variant's own fields sit as siblings of the discriminator, and the variant name
//! itself is never a JSON key (`{"format_type":"excel","sheet_count":2,...}`). Typed-language
//! e2e backends (python, rust, elixir, ruby) map such a segment to a real variant accessor and
//! need no help here. A backend that instead walks the parsed JSON tree by key — Zig's
//! `std.json.Value` lookup chain, Swift's `JSONSerialization` navigation of a swift-bridge
//! JSON-bridged leaf — must skip that segment entirely, or it looks up a key that does not
//! exist, the assertion reads `null`, and it passes while measuring nothing.
//!
//! The verdict is derived from the consumer crate's own IR, never from a list of names: see
//! [`FieldResolver::is_internally_tagged_variant_segment`]. This module previously carried one
//! consumer's `FormatMetadata` variant list plus the literal field name `format`, which is
//! silently wrong for a 22nd variant, for a differently named field, and for any second
//! internally tagged enum in the same crate.

use heck::ToUpperCamelCase;

use super::ir_enum::enum_type_at_path;
use super::types::FieldResolver;

/// Append one bracket-free segment to a dotted owner path, so a caller walking a fixture path
/// can keep [`FieldResolver::is_internally_tagged_variant_segment`]'s anchor up to date.
///
/// Brackets are the caller's to strip: the IR walk resolves by field name and sees through
/// `Option`/`Vec`, so `results[0]` and `results` must anchor identically. ~keep
pub(crate) fn push_owner_segment(owner_path: &mut String, bare_segment: &str) {
    if !owner_path.is_empty() {
        owner_path.push('.');
    }
    owner_path.push_str(bare_segment);
}

impl FieldResolver {
    /// Whether `segment` names a wire variant of an internally tagged enum declared as the type
    /// of the field at `owner_path` — the shape a JSON-navigating backend must skip rather than
    /// look up as a key.
    ///
    /// `owner_path` is the dotted, bracket-free path of the segment immediately preceding
    /// `segment`, anchored at the call's declared result type (the anchor
    /// [`super::types::IrEnumMap::root_type`] carries). The question asked is therefore what
    /// TYPE the preceding field has, never what that field is called.
    ///
    /// Answers `false` — never "unknown" — whenever the IR cannot positively confirm the shape:
    /// no anchored root type, a segment the IR does not know as a field on the current owner, a
    /// field whose type is not an enum, or an enum serde does not represent with an internal
    /// tag. Every one of those leaves the caller emitting the literal key lookup it emitted
    /// before any of this existed.
    ///
    /// ~keep The gate is the enum's REPRESENTATION, not
    /// [`crate::codegen::serde_enum_repr::serde_flattens_newtype_payload`]. That predicate
    /// answers the narrower "does serde merge this NEWTYPE variant's synthesized `_0` payload
    /// in", and is `false` for a unit variant and for a struct-shaped variant
    /// (`Variant { a: u8 }`) — both of which an internally tagged enum also writes with no key
    /// for the variant name, so gating on it would keep emitting a phantom key for exactly
    /// those two shapes. Under every other representation the payload does keep a real key
    /// (external: the variant name; adjacent: the content key; untagged: the bare payload), so
    /// the internal case is the only one that may be skipped.
    ///
    /// ~keep A fixture path may spell the variant either way, so both are accepted: its serde
    /// WIRE value (what `#[serde(rename_all = ...)]` produced, the only spelling the superseded
    /// hand-maintained list carried) or its Rust IDENTIFIER upper-camel-cased back, which is how
    /// [`FieldResolver::ir_tagged_union_split`] already reads a variant segment. For an enum
    /// renamed to `snake_case` the two coincide; for one with no rename, or a
    /// `SCREAMING_SNAKE_CASE`/`kebab-case` one, they do not, and accepting only the wire value
    /// would silently stop skipping.
    pub(crate) fn is_internally_tagged_variant_segment(&self, owner_path: &str, segment: &str) -> bool {
        if owner_path.is_empty() {
            return false;
        }
        let Some(enum_name) = enum_type_at_path(&self.ir_enum_map, owner_path) else {
            return false;
        };
        let Some(wire) = self.ir_enum_map.tagged_enum_wire.get(&enum_name) else {
            return false;
        };
        // `content` is `Some` only for adjacent tagging, which nests the payload under that key
        // instead of flattening it beside the tag — see `TaggedEnumWire::content`. ~keep
        if wire.content.is_some() {
            return false;
        }
        let identifier = segment.to_upper_camel_case();
        wire.variants
            .iter()
            .any(|(variant, value)| value == segment || *variant == identifier)
    }
}
