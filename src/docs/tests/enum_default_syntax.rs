//! Security/correctness control (consumer audit, task #558 follow-up): the extractor now
//! correctly withholds a fabricated default for an `Option<Enum>` field
//! (`extract::extractor::postprocess::resolve_enum_field_defaults`, `field.optional` guard), but
//! for a *required* enum field that legitimately documents its `#[default]` variant, the
//! generated docs must spell that variant the way the REAL backend actually declares it in each
//! target language -- not a plausible-looking guess. Before this fix,
//! `enum_variant_ref::format_enum_variant_ref` used one generic `{EnumType}.{Variant}` (or
//! `{EnumType}::{Variant}`) shape for every language, which does not parse as valid Go (no
//! separator between type and constant), does not name the real Java enum constant (Java declares
//! its constants SCREAMING_SNAKE), does not name the real Swift `case` or Dart member (both lowerCamelCase,
//! not PascalCase), and does not name the real PHP class constant (uppercased with no
//! underscores inserted). See `enum_variant_ref::format_enum_variant_ref`'s doc comment for the
//! exact backend call sites each arm below was verified against.

use super::*;
use crate::core::ir::{DefaultValue, EnumDef, EnumVariant};
use crate::docs::formatting::format_field_default;

/// Multi-word enum type and variant names so a wrong separator or a wrong casing rule cannot
/// coincidentally produce the same string as the correct one (unlike a short name such as
/// `Atx`, where `.to_uppercase()` and `.to_pascal_case()` collide).
fn render_mode_enum() -> EnumDef {
    EnumDef {
        name: "RenderMode".to_string(),
        has_default: true,
        variants: vec![
            EnumVariant {
                name: "PreferContent".to_string(),
                is_default: true,
                ..Default::default()
            },
            EnumVariant {
                name: "ContentOnly".to_string(),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

/// Exact per-language rendering of the `#[default]` variant `PreferContent` on enum
/// `RenderMode`, for a *required* field (so the `Empty`-typed-default branch documents the
/// enum's own default variant, mirroring `format_field_default`'s `!optional` guard).
const REQUIRED_FIELD_DEFAULT_BY_LANGUAGE: &[(Language, &str)] = &[
    // Go concatenates type and variant with no separator
    // (`backends/go/gen_bindings/types/enums.rs::go_enum_constant_for_wire_value`).
    (Language::Go, "`RenderModePreferContent`"),
    // Java declares its enum constants SCREAMING_SNAKE, its own convention
    // (`backends/java/gen_bindings/types/enums.rs::gen_enum_class`). This entry previously expected
    // the raw identifier and was accurate then -- the generator pushed `variant.name` verbatim, so
    // the docs faithfully described a generator that was itself non-idiomatic.
    (Language::Java, "`RenderMode.PREFER_CONTENT`"),
    // Swift's `case` is lowerCamelCase (`backends/swift/gen_bindings/enums.rs`).
    (Language::Swift, "`RenderMode.preferContent`"),
    // Dart's member is lowerCamelCase (`backends/dart/gen_bindings/types.rs`).
    (Language::Dart, "`RenderMode.preferContent`"),
    // PHP's class constant is the whole variant name uppercased, no underscores inserted
    // (`backends/php/gen_bindings/types/enums.rs::enum_constant_entries`).
    (Language::Php, "`RenderMode::PREFERCONTENT`"),
];

#[test]
fn required_enum_field_documents_default_variant_in_each_backends_real_syntax() {
    let api = ApiSurface {
        enums: vec![render_mode_enum()],
        ..make_minimal_api("0.1.0")
    };
    let field = make_field(
        "mode",
        TypeRef::Named("RenderMode".to_string()),
        false,
        Some(DefaultValue::Empty),
    );

    for (lang, expected) in REQUIRED_FIELD_DEFAULT_BY_LANGUAGE {
        let rendered = format_field_default(&field, *lang, &api, "Htm");
        assert_eq!(
            rendered, *expected,
            "docs for {lang:?} must spell the `RenderMode` default variant the way that \
             backend's own generator declares it; got `{rendered}`, expected `{expected}`. If \
             this fix were reverted, {lang:?}'s cell would render a syntax that does not parse \
             (or does not name a real constant) in the generated binding."
        );
    }
}

/// Same per-language spellings, but reached through an explicit qualified
/// `DefaultValue::EnumVariant("RenderMode::ContentOnly")` default rather than the `#[default]`
/// fallback -- the `splitn(2, "::")` branch of `format_typed_default`'s `EnumVariant` arm.
#[test]
fn explicit_qualified_enum_variant_default_uses_each_backends_real_syntax() {
    let api = ApiSurface {
        enums: vec![render_mode_enum()],
        ..make_minimal_api("0.1.0")
    };
    let field = make_field(
        "mode",
        TypeRef::Named("RenderMode".to_string()),
        false,
        Some(DefaultValue::EnumVariant("RenderMode::ContentOnly".to_string())),
    );

    let expected_by_language: &[(Language, &str)] = &[
        (Language::Go, "`RenderModeContentOnly`"),
        (Language::Java, "`RenderMode.CONTENT_ONLY`"),
        (Language::Swift, "`RenderMode.contentOnly`"),
        (Language::Dart, "`RenderMode.contentOnly`"),
        (Language::Php, "`RenderMode::CONTENTONLY`"),
    ];

    for (lang, expected) in expected_by_language {
        let rendered = format_field_default(&field, *lang, &api, "Htm");
        assert_eq!(
            rendered, *expected,
            "explicit qualified default for {lang:?}; got `{rendered}`"
        );
    }
}

/// Negative control closing the loop with the sibling `optional_enum_default` suite: an
/// `Option<RenderMode>` field with no explicit default must still render as absent (`` `null` ``
/// / `` `nil` ``), never as the enum's own default variant, in every one of these five
/// languages. A regression that reintroduced the old generic `{EnumType}.{Variant}` renderer
/// without also restoring the `field.optional` guard in `format_field_default`'s `Empty` branch
/// would fail this test by printing `RenderMode.PreferContent`-shaped text instead.
#[test]
fn optional_enum_field_in_the_five_fixed_languages_documents_absence_not_a_variant() {
    let api = ApiSurface {
        enums: vec![render_mode_enum()],
        ..make_minimal_api("0.1.0")
    };
    let field = make_field(
        "mode",
        TypeRef::Named("RenderMode".to_string()),
        true,
        Some(DefaultValue::Empty),
    );

    let nullish_by_language: &[(Language, &str)] = &[
        (Language::Go, "`nil`"),
        (Language::Java, "`null`"),
        (Language::Swift, "`null`"),
        (Language::Dart, "`null`"),
        (Language::Php, "`null`"),
    ];

    for (lang, expected) in nullish_by_language {
        let rendered = format_field_default(&field, *lang, &api, "Htm");
        assert_eq!(
            rendered, *expected,
            "optional enum field for {lang:?} must document absence, not the default variant; \
             got `{rendered}`"
        );
    }
}
