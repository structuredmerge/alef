//! task #559: `gen_enum`'s `Default` impl for a `#[napi(string_enum)]` wrapper used to be gated
//! on only the FIRST declared variant's own `cfg`. An enum whose variants are individually
//! feature-gated (one variant per selectable algorithm, e.g. `KeywordAlgorithm`) never carries a
//! `cfg` on the enum type itself -- only on its variants -- so the enum always exists, but the
//! whole `Default` impl vanished the moment the first-declared variant's feature was off, even
//! when a LATER variant's feature (and therefore a valid default value) was on. A struct field of
//! this enum type carrying `#[derive(Default)]` then fails to compile in that configuration with
//! "the trait bound `Default` is not satisfied".
//!
//! See `super::gen_enum`'s `default_impl_cfg_cascade` for the fix: one candidate `impl Default`
//! per declared variant, each candidate's guard excluding every earlier-declared variant's own
//! cfg, so exactly one candidate compiles under any feature combination that leaves at least one
//! declared variant enabled.

use super::gen_enum;
use crate::core::ir::{EnumDef, EnumVariant};

/// Build a host-owned enum whose variants each carry the given `cfg` (mirrors task #559's
/// reported shape: an enum whose variants are individually feature-gated, e.g. one variant per
/// selectable algorithm). `None` means the variant is declared unconditionally.
fn host_enum_with_gated_variants(name: &str, variant_cfgs: &[(&str, Option<&str>)]) -> EnumDef {
    EnumDef {
        name: name.to_string(),
        rust_path: format!("test::{name}"),
        variants: variant_cfgs
            .iter()
            .map(|(variant_name, cfg)| EnumVariant {
                name: (*variant_name).to_string(),
                cfg: cfg.map(|c| c.to_string()),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// Decisive case: the enum is fully declared (both variants present, matching a host-owned
/// enum's unconditional declaration -- see `codegen::conversions::enum_variant_declaration`'s
/// host-owned branch, which never drops a variant statically) while only the SECOND variant's
/// feature is actually on at build time. A fixture exercising only the all-features configuration
/// would pass under the old, broken code too, since the first variant's cfg is satisfied there --
/// this is why that configuration alone cannot prove the fix. ~keep
#[test]
fn gen_enum_default_impl_reachable_when_only_later_variant_feature_enabled() {
    let enum_def = host_enum_with_gated_variants(
        "SearchAlgorithm",
        &[
            ("Alpha", Some(r#"feature = "algo-alpha""#)),
            ("Beta", Some(r#"feature = "algo-beta""#)),
        ],
    );

    let output = gen_enum(&enum_def, "Js", true, "test", None, &[]);

    assert_eq!(
        output.matches("impl Default for JsSearchAlgorithm {").count(),
        2,
        "every declared variant needs its own candidate Default impl so the impl survives \
         whichever single variant's feature ends up enabled, got:\n{output}"
    );
    assert!(
        output.contains(
            "#[cfg(feature = \"algo-alpha\")]\n#[allow(clippy::derivable_impls)]\nimpl Default for JsSearchAlgorithm {\n    fn default() -> Self { Self::Alpha }\n}"
        ),
        "the first-declared variant's candidate must stay gated on exactly its own feature, got:\n{output}"
    );
    assert!(
        output.contains(
            "#[cfg(all(feature = \"algo-beta\", not(any(feature = \"algo-alpha\"))))]\n#[allow(clippy::derivable_impls)]\nimpl Default for JsSearchAlgorithm {\n    fn default() -> Self { Self::Beta }\n}"
        ),
        "the second-declared variant's candidate must be reachable whenever the first variant's \
         feature is off, so a build enabling only the second variant's feature still gets a \
         compilable Default impl, got:\n{output}"
    );
}

/// Table-driven: as more gated variants are declared, each later candidate's guard must
/// accumulate a `not(any(...))` over every earlier variant's own cfg, so the cascade stays
/// mutually exclusive (exactly one candidate can ever compile) no matter how many feature-gated
/// variants the enum has. Covers 2, 3, and 4 variants rather than asserting only the two-variant
/// shape, since a fix that special-cased "exactly two variants" would still leave a three-variant
/// enum (a very plausible next `KeywordAlgorithm`-style addition) broken the same way task #559
/// reported. ~keep
#[test]
fn gen_enum_default_impl_cascade_accumulates_negation_across_many_gated_variants() {
    let cases: &[&[&str]] = &[
        &["feature = \"a\"", "feature = \"b\""],
        &["feature = \"a\"", "feature = \"b\"", "feature = \"c\""],
        &[
            "feature = \"a\"",
            "feature = \"b\"",
            "feature = \"c\"",
            "feature = \"d\"",
        ],
    ];

    for cfgs in cases {
        let variant_names: Vec<String> = (0..cfgs.len()).map(|i| format!("V{i}")).collect();
        let variant_cfgs: Vec<(&str, Option<&str>)> = variant_names
            .iter()
            .zip(cfgs.iter())
            .map(|(name, cfg)| (name.as_str(), Some(*cfg)))
            .collect();
        let enum_def = host_enum_with_gated_variants("SearchAlgorithm", &variant_cfgs);

        let output = gen_enum(&enum_def, "Js", true, "test", None, &[]);

        assert_eq!(
            output.matches("impl Default for JsSearchAlgorithm {").count(),
            cfgs.len(),
            "every gated variant needs its own candidate ({} variants), got:\n{output}",
            cfgs.len()
        );

        let mut prior: Vec<&str> = Vec::new();
        for (i, cfg) in cfgs.iter().enumerate() {
            let expected_cfg = if prior.is_empty() {
                (*cfg).to_string()
            } else {
                format!("all({cfg}, not(any({})))", prior.join(", "))
            };
            let expected_block = format!(
                "#[cfg({expected_cfg})]\n#[allow(clippy::derivable_impls)]\nimpl Default for JsSearchAlgorithm {{\n    fn default() -> Self {{ Self::V{i} }}\n}}"
            );
            assert!(
                output.contains(&expected_block),
                "variant V{i} (of {} total) must carry the guard `{expected_cfg}`, got:\n{output}",
                cfgs.len()
            );
            prior.push(cfg);
        }
    }
}

/// A declared variant with NO `cfg` at all (unconditionally present, e.g. a foreign variant alef
/// cannot prove absent) always satisfies its own guard once reached in declaration order, so
/// nothing declared after it can ever be needed as a Default fallback -- the cascade must stop
/// there instead of emitting a dead-weight third candidate that can never be selected. ~keep
#[test]
fn gen_enum_default_impl_cascade_terminates_at_unconditional_variant() {
    let enum_def = host_enum_with_gated_variants(
        "SearchAlgorithm",
        &[
            ("Alpha", Some(r#"feature = "algo-alpha""#)),
            ("Beta", None),
            ("Gamma", Some(r#"feature = "algo-gamma""#)),
        ],
    );

    let output = gen_enum(&enum_def, "Js", true, "test", None, &[]);

    assert_eq!(
        output.matches("impl Default for JsSearchAlgorithm {").count(),
        2,
        "the cascade must stop at the first unconditionally-present variant (Beta); Gamma can \
         never be needed as a fallback and must not get its own candidate, got:\n{output}"
    );
    assert!(
        !output.contains("Self::Gamma }"),
        "a variant declared after an unconditional one must never appear in a Default body, got:\n{output}"
    );
    assert!(
        output.contains(
            "#[cfg(not(any(feature = \"algo-alpha\")))]\n#[allow(clippy::derivable_impls)]\nimpl Default for JsSearchAlgorithm {\n    fn default() -> Self { Self::Beta }\n}"
        ),
        "the unconditional variant's candidate must be reachable whenever every earlier candidate \
         is off, got:\n{output}"
    );
}

/// Regression guard: an enum with no `cfg` anywhere (the overwhelmingly common case) must keep
/// emitting exactly the same single, unconditional `Default` impl as before this fix -- no stray
/// `#[cfg(...)]` guard and no extra candidates. ~keep
#[test]
fn gen_enum_default_impl_stays_unconditional_when_ungated() {
    let enum_def = host_enum_with_gated_variants("SearchAlgorithm", &[("Alpha", None), ("Beta", None)]);

    let output = gen_enum(&enum_def, "Js", true, "test", None, &[]);

    assert_eq!(
        output.matches("impl Default for JsSearchAlgorithm {").count(),
        1,
        "an ungated enum must keep a single Default impl, got:\n{output}"
    );
    assert!(
        output.contains(
            "#[allow(clippy::derivable_impls)]\nimpl Default for JsSearchAlgorithm {\n    fn default() -> Self { Self::Alpha }\n}"
        ),
        "an ungated enum's Default impl must stay unconditional (no #[cfg(...)] guard) and keep \
         defaulting to the first declared variant, got:\n{output}"
    );
    assert!(
        !output.contains("#[cfg("),
        "an ungated enum must not gain a #[cfg(...)] guard on its Default impl, got:\n{output}"
    );
}
