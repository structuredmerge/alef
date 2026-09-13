//! Swift `render_assertion`'s bracket-wildcard traversal arm (`links[].url`), split out of
//! `assertions.rs` at the concept boundary to keep that file under the repo's 1,000-line
//! file-modularization cap. Verbatim extraction: every item below is byte-identical to the block
//! it replaces, with `render_wildcard_assertion` raised to `pub(super)` so its one caller in
//! `assertions.rs` can still reach it. ~keep

use std::fmt::Write as FmtWrite;

use crate::e2e::codegen::field_skip::nested_wildcard_skip_line;
use crate::e2e::field_access::FieldResolver;
use crate::e2e::fixture::Assertion;

use super::accessors::{SwiftTraversalContains, swift_traversal_contains_assert};
use super::values::json_to_swift;

struct WildcardTraversal<'a> {
    array_part: &'a str,
    element_part: &'a str,
    full_field: &'a str,
    result_variable: &'a str,
    field_resolver: &'a FieldResolver,
}

impl<'a> WildcardTraversal<'a> {
    fn new(
        array_part: &'a str,
        element_part: &'a str,
        full_field: &'a str,
        result_variable: &'a str,
        field_resolver: &'a FieldResolver,
    ) -> Self {
        Self {
            array_part,
            element_part,
            full_field,
            result_variable,
            field_resolver,
        }
    }
}

/// Render an assertion whose field path traverses an array with `[].` (e.g. `links[].url`).
///
/// `dot` is the byte offset of the `[].` separator in `field`, so `field[..dot]` is the array
/// path and `field[dot + 3..]` the per-element sub-path.
///
/// The wildcard means "some element of the array satisfies this", which Swift expresses as
/// `array.contains(where: { ... })`. Only the assertion types with a predicate form get that
/// treatment; every other type is refused with a visible skip.
///
/// Refusing is deliberate. The alternative — falling through to the generic accessor — is not
/// "no traversal", it is a *different assertion*: the shared resolver lowers `foo[]` to
/// `PathSegment::ArrayField { index: 0 }`, so the emitted Swift reads `result.foo()[0].bar()`
/// and passes whenever element zero happens to match, while claiming to cover the whole array.
/// That is a false green, which is strictly worse than a gap you can see. `zig` refuses the
/// nested-wildcard case for exactly this reason (`zig/assertions.rs`), and the skip wording
/// here is the one the other twelve backends already emit, so a wildcard `equals` is a
/// recorded gap in every language rather than a Swift-only accidental pass. ~keep
pub(super) fn render_wildcard_assertion(
    out: &mut String,
    assertion: &Assertion,
    field: &str,
    dot: usize,
    result_var: &str,
    field_resolver: &FieldResolver,
) {
    let array_part = &field[..dot];
    let elem_part = &field[dot + 3..];
    let traversal = WildcardTraversal::new(array_part, elem_part, field, result_var, field_resolver);

    // The split above consumes the FIRST `[].` only, so a doubly-nested path leaves a second
    // wildcard in `elem_part`. The `not_empty` arm builds its element accessor inline instead of
    // going through `swift_traversal_contains_assert`, so without this guard `accessor` lowers that
    // surviving wildcard to index 0 and the `contains(where:)` closure ranges over `pages` while
    // reading `links[0]`. Guarding here rather than per-arm also gives the refused path the wording
    // the strict field-availability gate counts, which the generic `unsupported traversal
    // assertion` fallback deliberately is not. ~keep
    if let Some(line) = nested_wildcard_skip_line("        ", "//", field, elem_part) {
        let _ = writeln!(out, "{line}");
        return;
    }

    match assertion.assertion_type.as_str() {
        "contains" => {
            if let Some(expected) = &assertion.value {
                emit_wildcard_contains(out, expected, false, &traversal);
            }
        }
        "contains_all" => {
            if let Some(values) = &assertion.values {
                for value in values {
                    emit_wildcard_contains(out, value, false, &traversal);
                }
            }
        }
        "not_contains" => {
            for expected in assertion.expected_values() {
                emit_wildcard_contains(out, expected, true, &traversal);
            }
        }
        "not_empty" => {
            let array_accessor = field_resolver.accessor(array_part, "swift", result_var);
            let resolved_full = field_resolver.resolve(field);
            let resolved_elem_part = resolved_full
                .find("[].")
                .map(|d| &resolved_full[d + 3..])
                .unwrap_or(elem_part);
            let elem_accessor = field_resolver.element_accessor(resolved_elem_part, "swift", "$0");
            let elem_is_enum = field_resolver.is_enum(field);
            // A first-class payload-carrying enum has `.toString()` (see
            // `gen_bindings::enums::emit_swift_wire_tag_accessor`) but no `.rawValue` —
            // `swift_scalar_leaf_string_expr` needs this to pick the right one. ~keep
            let elem_is_data_carrying_enum = field_resolver.ir_enum_is_data_carrying(field).unwrap_or(false);
            let elem_is_optional = field_resolver.is_optional(resolved_elem_part)
                || field_resolver.is_optional(field_resolver.resolve(resolved_elem_part));
            let elem_str = super::leaf_shape::swift_scalar_leaf_string_expr(
                &elem_accessor,
                elem_is_enum,
                elem_is_data_carrying_enum,
                elem_is_optional,
            );
            let _ = writeln!(
                out,
                "        XCTAssertTrue({array_accessor}.contains(where: {{ !{elem_str}.isEmpty }}), \"expected non-empty value\")"
            );
        }
        other => {
            let _ = writeln!(
                out,
                "        // skipped: unsupported traversal assertion '{other}' on '{field}'"
            );
        }
    }
}

/// Emit one `XCTAssert{True,False}(array.contains(where: { … }), …)` line for a wildcard path.
fn emit_wildcard_contains(
    out: &mut String,
    value: &serde_json::Value,
    negate: bool,
    traversal: &WildcardTraversal<'_>,
) {
    let swift_val = json_to_swift(value);
    let msg = if negate {
        format!("expected NOT to contain: \\({swift_val})")
    } else {
        format!("expected to contain: \\({swift_val})")
    };
    let line = swift_traversal_contains_assert(SwiftTraversalContains {
        array_part: traversal.array_part,
        element_part: traversal.element_part,
        full_field: traversal.full_field,
        value_expression: &swift_val,
        result_variable: traversal.result_variable,
        negate,
        message: &msg,
        field_resolver: traversal.field_resolver,
    });
    let _ = writeln!(out, "{line}");
}

#[cfg(test)]
mod nested_wildcard_tests {
    use super::render_wildcard_assertion;
    use crate::e2e::field_access::FieldResolver;
    use crate::e2e::fixture::Assertion;
    use std::collections::{HashMap, HashSet};

    fn array_resolver(field: &str) -> FieldResolver {
        let names: HashSet<String> = [field.to_string()].into_iter().collect();
        FieldResolver::new(&HashMap::new(), &HashSet::new(), &names, &names, &HashSet::new())
    }

    fn render_not_empty(field: &str, resolver: &FieldResolver) -> String {
        let assertion = Assertion {
            assertion_type: "not_empty".to_string(),
            field: Some(field.to_string()),
            ..Assertion::default()
        };
        let dot = field.find("[].").expect("test field must carry a wildcard");
        let mut out = String::new();
        render_wildcard_assertion(&mut out, &assertion, field, dot, "result", resolver);
        out
    }

    /// The control, and the one that matters: a single wildcard must still quantify over every
    /// element, so the refusal below cannot have been implemented by disabling wildcards. ~keep
    #[test]
    fn single_wildcard_not_empty_still_quantifies_over_every_element() {
        let out = render_not_empty("links[].url", &array_resolver("links"));
        assert!(out.contains(".contains(where: {"), "got: {out}");
        assert!(!out.contains("skipped:"), "got: {out}");
        assert!(!out.contains("[0]"), "got: {out}");
    }

    /// `not_empty` was the arm the shared `swift_traversal_contains_assert` guard never covered:
    /// it builds its element accessor inline, so `pages[].links[].url` emitted a closure over
    /// `pages` whose body read `links[0]` — a whole-array claim inspecting one inner element.
    /// Pre-guard this test fails on the emitted `XCTAssertTrue(...contains(where:...))` line. ~keep
    #[test]
    fn nested_wildcard_not_empty_should_emit_a_visible_skip_rather_than_an_index_zero_check() {
        let out = render_not_empty("pages[].links[].url", &array_resolver("pages"));
        assert_eq!(
            out, "        // skipped: nested array-wildcard field 'pages[].links[].url' not supported\n",
            "got: {out}"
        );
    }
}
