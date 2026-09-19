use crate::codegen::keywords::swift_ident;
use crate::e2e::codegen::field_skip::nested_wildcard_skip_line;
use crate::e2e::field_access::FieldResolver;
use heck::ToLowerCamelCase;
use std::collections::HashMap;

/// Build a Swift accessor path for the given fixture field, inserting `()` on
/// every segment and `?` after every optional non-leaf segment.
///
/// This is the core helper for count/contains helpers that need to reconstruct
/// the path with correct optional chaining from the raw fixture field name.
///
/// Rewrite a Swift accessor expression to capture any `RustVec` temporaries
/// in a local before subscripting them. Returns `(setup_lines, rewritten_expr)`.
///
/// swift-bridge's `Vec_<T>$get` returns a raw pointer into the Vec's storage
/// wrapped in a `T.SelfRef`. If the Vec was a temporary, ARC may release it
/// before the ref is dereferenced, leaving the pointer dangling and reads
/// returning empty/garbage. Hoisting the Vec into a `let` binding ties the
/// Vec's lifetime to the enclosing function scope, so the ref stays valid.
///
/// Every `()[...]` or `()?[...]` occurrence in the expression is materialised in turn, left to
/// right, so a nested indexed chain (e.g. `result.items()[0].nested()[1].value()`, produced when
/// a field-access path traverses two `RustVec`-backed segments) hoists BOTH temporaries. Hoisting
/// only the outer one still leaves the inner `.nested()` call — itself a fresh `RustVec`
/// temporary — subscripted inline, which reproduces the exact dangling-pointer hazard this
/// function exists to prevent, just one level deeper. Each hoist's `let` reads from the previous
/// hoist's local (or from the original expression for the first), so the locals chain correctly.
///
/// ~keep Hoisting an OPTIONAL subscript into a `let` changes what is optional and where. The
/// `let` RHS never includes a trailing `?` — `let x = a?` alone is not valid Swift, `?` must be
/// immediately followed by a member/subscript access — so the `?` is re-inserted in the
/// REWRITTEN expression right after the local name, before its subscript (`local?[N]`, never
/// `local[N]`). Once a chain crosses ONE optional point, every LATER hoist in the SAME call must
/// also emit `?`, even when that later hoist's OWN marker in the source text is a plain `()[`:
/// the original generator writes the FIRST `?` only and lets Swift's optional chaining
/// auto-propagate through everything textually after it in ONE continuous expression (`a?.b.c`
/// needs no second `?` before `.c`). Splitting that expression across separate `let` bindings
/// breaks the continuity — `let x = a?.b; x.c` is a hard compile error, `x` is now a standalone
/// `Optional<B>` and plain `.c` on an Optional does not compile; `x?.c` is required. A
/// `carried_optional` flag tracks this: once any earlier hoist's own marker was optional, every
/// later hoist emits `?` regardless of its own marker. A string-key (map) hoist is the one
/// exception — its `let` always resolves through `?? [:]`, so the local is never optional and
/// never gets a `?`; see `build_hoist_setup`. ~keep
///
/// Refusal (`None`) is the SAME posture for two different failures: a malformed subscript with
/// no matching `]` at all, and a mixed map-then-vec chain (a string-key subscript followed by a
/// further `()[`/`()?[` later in the tail — decoding a map value never yields anything a further
/// `RustVec` hoist can act on). Both return `None` rather than a partially-rewritten expression,
/// so a caller has exactly one way to learn "this could not be safely rewritten." Otherwise
/// returns `Some((setup_lines, rewritten_expr, is_map_subscript))`, where `is_map_subscript` is
/// true when the LAST subscript's key was a string literal, indicating the deepest accessor
/// returns a JSON-encoded Map (RustString) and the rewritten expression already evaluates to
/// `String?` so callers should NOT append `.toString()`. ~keep
pub(super) fn materialise_vec_temporaries(expr: &str, name_suffix: &str) -> Option<(Vec<String>, String, bool)> {
    let mut setups = Vec::new();
    let mut current = expr.to_string();
    let mut is_string_key = false;
    let mut hoist_count = 0usize;
    let mut carried_optional = false;

    while let Some((idx, marker_optional)) = find_next_subscript_marker(&current) {
        let is_optional = marker_optional || carried_optional;
        hoist_count += 1;
        match hoist_one_subscript(&current, idx, marker_optional, is_optional, name_suffix, hoist_count) {
            HoistOutcome::Refuse => return None,
            HoistOutcome::Hoisted {
                setup,
                next,
                is_string_key: key,
            } => {
                setups.push(setup);
                current = next;
                is_string_key = key;
                carried_optional = carried_optional || marker_optional;
            }
        }
    }

    Some((setups, current, is_string_key))
}

/// The result of attempting to hoist the ONE subscript marker located at a known position.
/// See [`materialise_vec_temporaries`]'s doc for why both failure shapes below collapse to the
/// same `Refuse` outcome rather than two different ones.
enum HoistOutcome {
    Hoisted {
        setup: String,
        next: String,
        is_string_key: bool,
    },
    Refuse,
}

/// Process the subscript marker [`find_next_subscript_marker`] already located at `idx`: find
/// its closing bracket, split the expression into prefix/subscript/tail, and either hoist it
/// into a `let` or refuse the whole expression when it cannot be safely rewritten.
///
/// A string-key subscript (e.g. `["title"]`) signals Map-like access — swift-bridge serialises
/// non-leaf Maps as JSON-encoded `RustString`, decoded by [`build_hoist_setup`]. Once decoded,
/// the value is a plain Swift `String` with nothing a further `RustVec` hoist can act on, so a
/// string-key subscript followed by another `()[`/`()?[` in the tail refuses (`Refuse`) rather
/// than emitting broken Swift — the IR-backed `json_bridged_traversal_skip` (leaf_shape.rs)
/// catches this earlier when it has IR data; a config-only/opaque resolver never does, so this
/// is the fallback net. Map hoists never carry a trailing `?` (their local is never optional —
/// see `build_hoist_setup`); a plain vec hoist carries one exactly when `is_optional`.
fn hoist_one_subscript(
    current: &str,
    idx: usize,
    marker_optional: bool,
    is_optional: bool,
    name_suffix: &str,
    hoist_count: usize,
) -> HoistOutcome {
    let bracket_start = if marker_optional { idx + 3 } else { idx + 2 }; // `?[` adds one byte ~keep
    let after_open = bracket_start + 1; // first char inside the brackets ~keep
    let Some(close_rel) = find_subscript_close(&current[after_open..]) else {
        return HoistOutcome::Refuse;
    };
    let subscript_end = after_open + close_rel; // index of `]` ~keep
    let prefix = &current[..idx + 2]; // includes `()`, never the trailing `?` ~keep
    let subscript = &current[bracket_start..=subscript_end]; // `[N]` or `["key"]` ~keep
    let tail = &current[subscript_end + 1..]; // everything after `]` ~keep
    let method_dot = current[..idx].rfind('.').unwrap_or(0);
    let method = &current[method_dot + 1..idx];
    let local = format!("_vec_{method}_{name_suffix}_{hoist_count}");

    let inner = subscript.trim_start_matches('[').trim_end_matches(']');
    let is_string_key = inner.starts_with('"') && inner.ends_with('"');
    if is_string_key && (tail.contains("()[") || tail.contains("()?[")) {
        return HoistOutcome::Refuse;
    }

    let setup = build_hoist_setup(&local, prefix, is_string_key, is_optional);
    let next = if !is_string_key && is_optional {
        format!("{local}?{subscript}{tail}")
    } else {
        format!("{local}{subscript}{tail}")
    };

    HoistOutcome::Hoisted {
        setup,
        next,
        is_string_key,
    }
}

/// Build the `let` line for one hoisted temporary.
///
/// ~keep A string-key (map) subscript decodes its prefix's JSON-bridged `RustString` getter into
/// `[String: String]`; when that getter is itself reached through an optional chain
/// (`is_optional`), the decode must call `?.toString()` rather than `.toString()` — the prefix's
/// type is `Optional<RustString>` at that point, and a plain `.toString()` on an Optional does
/// not compile. A plain (non-map) hoist just captures whatever `prefix` evaluates to, optional or
/// not — assigning an Optional expression to a `let` needs no unwrap, so no such branch exists
/// for it.
fn build_hoist_setup(local: &str, prefix: &str, is_string_key: bool, is_optional: bool) -> String {
    if !is_string_key {
        return format!("let {local} = {prefix}");
    }
    let to_string_call = if is_optional {
        format!("{prefix}?.toString()")
    } else {
        format!("{prefix}.toString()")
    };
    format!(
        "let {local} = (try? JSONSerialization.jsonObject(with: ({to_string_call} ?? \"{{}}\").data(using: .utf8)!) as? [String: String]) ?? [:]"
    )
}

/// Find the next `()[` or `()?[` subscript-open marker in `s`, ignoring any such text that
/// occurs INSIDE an already-quoted string-key subscript. Returns `(start_index, is_optional)`.
///
/// ~keep A map key's own content can legitimately contain the literal text `()[` — nothing
/// escapes brackets when `quoted_key_literal` writes a key out (only `\`, `"`, and whitespace
/// control characters are escaped). Once hoisted, that content sits verbatim inside `current`'s
/// rewritten quoted subscript, e.g. `_vec_labels_X_1["a()[b"]`. A quote-BLIND scan for `()[` on
/// the next loop iteration would match the fake occurrence embedded in that key before reaching
/// a real subsequent subscript — or, when the key was the terminal subscript, would find a fake
/// "next marker" where none exists at all, splitting the key's own content into garbage. Tracking
/// whether the scan is currently inside a quoted region (toggling on unescaped `"`, skipping the
/// byte after `\`) confines every pattern match to text that is actually expression structure,
/// never a key's payload. This only finds where a marker STARTS; [`find_subscript_close`] (run
/// afterward, on content immediately following the opening `[`) finds where that ONE subscript's
/// content ENDS — a narrower, already-open-bracket job with no outer quote-tracking of its own.
fn find_next_subscript_marker(s: &str) -> Option<(usize, bool)> {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut in_quotes = false;
    while i < bytes.len() {
        if in_quotes {
            match bytes[i] {
                b'\\' => i += 2,
                b'"' => {
                    in_quotes = false;
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }
        if bytes[i] == b'"' {
            in_quotes = true;
            i += 1;
        } else if bytes[i..].starts_with(b"()?[") {
            return Some((i, true));
        } else if bytes[i..].starts_with(b"()[") {
            return Some((i, false));
        } else {
            i += 1;
        }
    }
    None
}

/// Find the byte offset (relative to `content`, which starts right after a subscript's opening
/// `[`) of the `]` that closes THIS subscript, given an already-correct starting position.
///
/// ~keep A naive `content.find(']')` closes the subscript at the FIRST `]` in the content, which
/// is wrong whenever the key itself contains a `]` (e.g. `labels["a]b"]` — `quoted_key_literal`
/// escapes `\`, `"`, and whitespace control characters, but never `]`). The naive scan sees the
/// `]` inside `"a]b"` and stops there, leaving the subscript malformed (`["a]`, missing its
/// closing quote) and the misclassified `is_string_key` check (which trims one leading `[` and
/// one trailing `]`, then checks both quote ends) sees `"a` — starts with `"` but does not end
/// with one — so it reads as a NUMERIC subscript and skips the JSON-decode setup entirely.
/// Scanning quote-aware — treating a leading `"` as the start of a string that runs to the next
/// unescaped `"`, only then searching for the closing `]` — finds the true boundary of THIS
/// subscript regardless of what its key contains. Unlike [`find_next_subscript_marker`], this
/// function only ever sees content starting exactly at an open bracket, so it has no reason to
/// distinguish real structure from a PREVIOUSLY-hoisted key's content sitting earlier in the
/// string — that is a property of scanning arbitrary already-rewritten text for the NEXT
/// marker's start, not of finding one already-located subscript's end.
fn find_subscript_close(content: &str) -> Option<usize> {
    let bytes = content.as_bytes();
    if bytes.first() != Some(&b'"') {
        return content.find(']');
    }
    let mut i = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return content[i + 1..].find(']').map(|rel| i + 1 + rel),
            _ => i += 1,
        }
    }
    None
}

// ~keep `swift_build_accessor` and its per-segment helpers moved to `accessor_walk.rs` (this
// file was approaching the file-size-ratchet cap); re-exported here so every existing
// `super::accessors::swift_build_accessor` caller keeps working unchanged.
pub(super) use super::accessor_walk::swift_build_accessor;

pub(super) struct SwiftTraversalContains<'a> {
    pub(super) array_part: &'a str,
    pub(super) element_part: &'a str,
    pub(super) full_field: &'a str,
    pub(super) value_expression: &'a str,
    pub(super) result_variable: &'a str,
    pub(super) negate: bool,
    pub(super) message: &'a str,
    pub(super) field_resolver: &'a FieldResolver,
}

/// Generate a `[String]` (or `[String]?`) expression for a `RustVec<RustString>`
/// field so that `contains` membership checks work against plain Swift Strings.
///
/// We use `.map { $0.asStr().toString() }` because:
/// 1. Iterating a `RustVec<RustString>` yields `RustStringRef` (not `RustString`), which
///    only has `asStr()` but not `toString()` directly. swift-bridge auto-renames the
///    Rust `as_str` method to lowerCamelCase `asStr` on the Swift side.
/// 2. The accessor may end with an `Optional<RustVec<RustString>>` (e.g. `sheet_names()` is
///    `Option<Vec<String>>` in Rust, which becomes `Optional<RustVec<RustString>>` in Swift).
/// 3. Optional chaining from parent `?.` already produces `Optional<RustVec<T>>`.
///
/// The returned tuple's bool indicates whether the result is `Optional<[String]>`
/// (callers coalesce with `?? []`) or already a concrete `[String]`. Emitting
/// `?? []` against a non-optional value compiles with a Swift warning but is
/// surfaced as an error in strict CI configurations, so we only emit `?.map`
/// + `?? []` when the accessor is genuinely optional.
///
/// Generate a `XCTAssert{True|False}(array.contains(where: { elem_str.contains(val) }), msg)` line
/// for field paths that traverse a collection with `[].` notation (e.g. `links[].url`).
///
/// `array_part` — left side of `[].` (e.g. `"links"`)
/// `element_part` — right side (e.g. `"url"` or `"link_type"`)
/// `full_field` — original assertion.field (used for enum lookup against the full path)
pub(super) fn swift_traversal_contains_assert(context: SwiftTraversalContains<'_>) -> String {
    let array_accessor = context
        .field_resolver
        .accessor(context.array_part, "swift", context.result_variable);
    let resolved_full = context.field_resolver.resolve(context.full_field);
    let resolved_elem_part = resolved_full
        .find("[].")
        .map(|d| &resolved_full[d + 3..])
        .unwrap_or(context.element_part);
    // The split above consumes the first `[].` only, so a doubly-nested path leaves a second
    // wildcard in the element sub-path that `accessor` would lower to index 0 — the
    // `contains(where:)` closure would then range over `pages` while reading `links[0]`. Return
    // the refusal as this function's rendered line; every caller writes the return value out. ~keep
    if let Some(line) = nested_wildcard_skip_line("        ", "//", context.full_field, resolved_elem_part) {
        return line;
    }
    let elem_accessor = context
        .field_resolver
        .element_accessor(resolved_elem_part, "swift", "$0");
    // `field_resolver.is_enum` consults the hand-maintained `fields_enum` config first and falls
    // back to the IR-derived classification when the config is silent — see `render_assertion`'s
    // `field_is_enum` comment for the failure mode a config-only check produced. ~keep
    let elem_is_enum = context.field_resolver.is_enum(context.full_field);
    // A payload-carrying enum has no `.rawValue` once promoted (only the all-unit shape gets
    // one), but it DOES have `.toString()` -- `gen_bindings::enums::emit_swift_wire_tag_accessor`
    // gives every promoted payload-carrying enum a `toString()` returning the same serde wire tag
    // the pre-promotion opaque mirror's `to_string()` always returned. `unwrap_or(false)` is the
    // safe default for a field the IR does not positively resolve to a concrete enum (config-only
    // `fields_enum` classification): `elem_is_enum` is still true from the config, and the
    // `.toString()` fallback below is correct either way. ~keep
    let elem_is_data_carrying_enum = context
        .field_resolver
        .ir_enum_is_data_carrying(context.full_field)
        .unwrap_or(false);
    let elem_is_optional = context.field_resolver.is_optional(resolved_elem_part)
        || context
            .field_resolver
            .is_optional(context.field_resolver.resolve(resolved_elem_part));
    // `swift_scalar_leaf_string_expr` picks `.toString()` (method-call/opaque leaf, or a
    // first-class payload-carrying enum) vs `.rawValue` (a first-class all-unit enum) vs bare
    // access (a first-class plain `String` leaf, which has neither) from the accessor
    // expression's own shape — see that function's doc for why a single call answers correctly
    // for every combination. ~keep
    let elem_str = super::leaf_shape::swift_scalar_leaf_string_expr(
        &elem_accessor,
        elem_is_enum,
        elem_is_data_carrying_enum,
        elem_is_optional,
    );
    let assert_fn = if context.negate {
        "XCTAssertFalse"
    } else {
        "XCTAssertTrue"
    };
    format!(
        "        {assert_fn}({array_accessor}.contains(where: {{ {elem_str}.contains({}) }}), \"{}\")",
        context.value_expression, context.message
    )
}

/// Returns `(map_expr, is_optional)` where `map_expr` is the `.map { … }` chain
/// that converts each element to a Swift `String`, and `is_optional` reports
/// whether the resulting expression is `Optional<[String]>` (callers should
/// coalesce with `?? []`) or already a concrete `[String]`.
///
/// When `materialized_expr` is provided (from a prior call to `materialise_vec_temporaries`),
/// use that expression instead of rebuilding the accessor. This keeps RustVec temporaries
/// bound to locals, preventing use-after-free when swift-bridge releases them.
pub(super) fn swift_array_contains_expr(
    field: Option<&str>,
    result_var: &str,
    field_resolver: &FieldResolver,
    result_field_accessor: &HashMap<String, String>,
    materialized_expr: Option<&str>,
) -> (String, bool) {
    // swift-bridge auto-renames Rust snake_case methods to lowerCamelCase on the
    // Swift side. `RustStringRef::as_str()` is exposed as `asStr()` — emitting
    // `as_str()` produces "value of type 'XRef' has no member 'as_str'" at
    // compile time.
    let Some(f) = field else {
        return (format!("{result_var}.map {{ $0.asStr().toString() }}"), false);
    };
    // Allow per-call overrides to name a different element accessor — used when
    // the array element is an opaque struct whose "name string" accessor is
    // not `as_str` (e.g. `StructureItem` exposes `kind() -> String`). The map
    // is keyed on the fixture field name (and resolved alias as a fallback).
    let resolved_field = field_resolver.resolve(f);
    let elem_accessor_name = result_field_accessor
        .get(f)
        .or_else(|| result_field_accessor.get(resolved_field))
        .cloned()
        .unwrap_or_else(|| "as_str".to_string());
    let elem_call = swift_ident(&elem_accessor_name.to_lower_camel_case());
    // When a materialized expression is provided (from materialise_vec_temporaries),
    // use it directly instead of rebuilding. This keeps RustVec temporaries bound.
    let (accessor, has_optional) = if let Some(expr) = materialized_expr {
        (expr.to_string(), swift_build_accessor(f, result_var, field_resolver).1)
    } else {
        swift_build_accessor(f, result_var, field_resolver)
    };
    // Only chain `?.map` when the accessor is actually optional. The previous
    // unconditional `?.map` produced "cannot use optional chaining on
    // non-optional value of type 'RustVec<…>'" for plain `Vec<T>` fields.
    let field_is_optional =
        has_optional || field_resolver.is_optional(f) || field_resolver.is_optional(field_resolver.resolve(f));
    if field_is_optional {
        (format!("{accessor}?.map {{ $0.{elem_call}().toString() }}"), true)
    } else {
        (format!("{accessor}.map {{ $0.{elem_call}().toString() }}"), false)
    }
}

/// Emit a `XCTAssertTrue(array.contains(where: { ... }), msg)` line that
/// aggregates every text-bearing accessor on the element type of a `Vec<T>`
/// field, mirroring python's `_alef_e2e_item_texts` helper.
///
/// Returns `None` when:
///   - `field` is missing
///   - The field's root or leaf type cannot be resolved
///   - The element type has fewer than 2 stringy fields (the existing
///     single-accessor path is good enough and emits simpler code)
///
/// When matched, emits a closure that gathers `source().toString()`,
/// `items().map { $0.asStr().toString() }`, `alias()?.toString()`, etc. into
/// a flat `[String]` and substring-matches the expected value against every
/// entry. The matcher is lenient so that fixtures asserting `"os"` against
/// the `imports` field — where `ImportInfo.source` may be the bare module
/// name (`"os"`), the entire import statement (`"import os"`), or the
/// imported items (`from os import path` → items=["path"]) — succeed
/// regardless of how the language extractor surfaces the value.
pub(super) fn swift_stringy_aggregator_contains_assert(
    field: Option<&str>,
    result_var: &str,
    field_resolver: &FieldResolver,
    swift_val: &str,
) -> Option<String> {
    let field = field?;
    let resolved = field_resolver.resolve(field);
    // Only handle simple top-level array fields (no nested chains) for now.
    // Field path containing `.` or `[` is left to the existing traversal/array
    // paths.
    if resolved.contains('.') || resolved.contains('[') {
        return None;
    }
    let root_type = field_resolver.swift_root_type()?.clone();
    let elem_type = field_resolver.swift_advance(Some(&root_type), resolved)?;
    let stringy = field_resolver.swift_stringy_fields(&elem_type)?;
    if stringy.len() < 2 {
        return None;
    }
    let array_accessor = field_resolver.accessor(field, "swift", result_var);
    // ~keep `stringy_fields_by_type` classifies enum-typed fields as "stringy" assuming the
    // OPAQUE (method-call) lowering, where every enum getter -- unit or payload-carrying -- is
    // bridged to a `RustString` (`swift/values.rs`'s `classify_stringy` doc). Once `elem_type`
    // is first-class (property-access), that assumption breaks: `item.field()` becomes bare
    // `item.field`, and an enum leaf needs `.rawValue` (unit) or has NO scalar accessor at all
    // (payload-carrying) -- see `stringy_field_text_line`.
    //
    // ~keep The element's own promotion is necessary but not sufficient. The closure's `item`
    // is whatever `{array_accessor}` yields, and that is decided by the ROOT: a first-class
    // root stores `items` as a Swift `[T]` (subscript → first-class `T`), but an opaque
    // (`typealias`-to-`RustBridge`) root only has an `items()` getter returning
    // `RustVec<RustBridge.T>`, whose elements expose swift-bridge METHODS regardless of `T`
    // being independently promoted — the same rule `accessor_walk`'s `via_opaque` pins for
    // traversal chains. A consumer's opaque root with a promoted element type rendered
    // `item.name` against a `<T>Ref` and failed to compile.
    let is_first_class =
        field_resolver.swift_is_first_class(Some(&root_type)) && field_resolver.swift_is_first_class(Some(&elem_type));
    let mut texts_lines: Vec<String> = Vec::new();
    for sf in stringy {
        if let Some(line) = stringy_field_text_line(field_resolver, &elem_type, is_first_class, sf) {
            texts_lines.push(line);
        }
    }
    // Every candidate line dropped (e.g. every stringy field turned out to be a first-class
    // payload-carrying enum with no scalar accessor) leaves nothing to aggregate on. Refusing
    // here lets the caller fall back to its other paths instead of emitting a closure whose body
    // never appends anything — a `contains` that can never be true and a `not_contains` that can
    // never be false, i.e. an assertion that reads as coverage but checks nothing. ~keep
    if texts_lines.is_empty() {
        return None;
    }
    let texts_block = texts_lines.join("\n");
    Some(format!(
        "        XCTAssertTrue({array_accessor}.contains(where: {{ item in\n            var texts = [String]()\n{texts_block}\n            return texts.contains(where: {{ $0.contains({swift_val}) }})\n        }}), \"expected to contain: \\({swift_val})\")"
    ))
}

/// ~keep One `texts.append(...)` line for a single stringy field of a `contains`-aggregated
/// element type, per its [`StringyFieldKind`] — the per-field body
/// [`swift_stringy_aggregator_contains_assert`]'s loop used to inline directly. `None` only for a
/// first-class payload-carrying enum field, which has no scalar accessor to append at all (see
/// the caller's doc).
fn stringy_field_text_line(
    field_resolver: &FieldResolver,
    elem_type: &str,
    is_first_class: bool,
    sf: &crate::e2e::field_access::StringyField,
) -> Option<String> {
    use crate::e2e::field_access::StringyFieldKind;
    let call = swift_ident(&sf.name.to_lower_camel_case());
    if !is_first_class {
        // Opaque (method-call) element: `item.field()` returns `RustVec<RustString>` for the
        // `Vec` kind. Mapping its elements yields `RustStringRef` — a swift-bridge wrapper around
        // the borrowed RustString — which has `as_str()` (snake_case, defined in
        // `SwiftBridgeCore.swift`), NOT `toString()` (only `RustString` has the latter via the
        // extension that calls `self.as_str().toString()`).
        return Some(match sf.kind {
            StringyFieldKind::Plain => format!("                texts.append(item.{call}().toString())"),
            StringyFieldKind::Optional => {
                format!("                if let v = item.{call}() {{ texts.append(v.toString()) }}")
            }
            StringyFieldKind::Vec => {
                format!("                texts.append(contentsOf: item.{call}().map {{ $0.as_str().toString() }})")
            }
        });
    }
    // First-class (property-access) element. `enum_shape` is `None` for a non-enum (plain
    // `String`) field, `Some(false)` for an all-unit enum (`.rawValue`), `Some(true)` for a
    // payload-carrying one. A payload-carrying enum has no `.rawValue`, but DOES have
    // `.toString()` — `gen_bindings::enums::emit_swift_wire_tag_accessor` gives every promoted
    // payload-carrying enum one, returning the same serde wire tag the field's opaque getter used
    // to return — so it contributes text too, just through a different accessor name. ~keep
    let enum_shape = field_resolver.ir_enum_shape_on_type(elem_type, &sf.name);
    let suffix = match enum_shape {
        None => "",
        Some(false) => ".rawValue",
        Some(true) => ".toString()",
    };
    Some(match sf.kind {
        StringyFieldKind::Plain => format!("                texts.append(item.{call}{suffix})"),
        StringyFieldKind::Optional => {
            format!("                if let v = item.{call} {{ texts.append(v{suffix}) }}")
        }
        StringyFieldKind::Vec if !suffix.is_empty() => {
            format!("                texts.append(contentsOf: item.{call}.map {{ $0{suffix} }})")
        }
        StringyFieldKind::Vec => format!("                texts.append(contentsOf: item.{call})"),
    })
}

/// Generate a `.count` expression for an array field that may be nested inside optional parents.
///
/// Swift-bridge exposes all Rust fields as methods with `()`. When ancestor segments are
/// optional, we use `?.` chaining. The final count is coalesced with `?? 0` when there
/// are optional ancestors so the XCTAssert macro receives a non-optional `Int`.
///
/// Also check if the field itself (the leaf) is optional, which happens when the field
/// returns Optional<RustVec<T>> (e.g., `links()` may return Optional).
///
/// When `materialized_expr` is provided (from a prior call to `materialise_vec_temporaries`),
/// use that expression instead of rebuilding the accessor. This keeps RustVec temporaries
/// bound to locals, preventing use-after-free when swift-bridge releases them.
///
/// Returns `None` when the field is actually a scalar String (not a collection) that was
/// incorrectly marked as an array in the e2e config. In this case, count assertions
/// should be skipped.
pub(super) fn swift_array_count_expr(
    field: Option<&str>,
    result_var: &str,
    field_resolver: &FieldResolver,
    materialized_expr: Option<&str>,
) -> Option<String> {
    let Some(f) = field else {
        return Some(format!("{result_var}.count"));
    };
    // When a materialized expression is provided (from materialise_vec_temporaries),
    // use it directly instead of rebuilding. This keeps RustVec temporaries bound.
    let accessor = if let Some(expr) = materialized_expr {
        expr.to_string()
    } else {
        swift_build_accessor(f, result_var, field_resolver).0
    };
    let mut has_optional = swift_build_accessor(f, result_var, field_resolver).1;
    // Also check if the leaf field itself is optional.
    if field_resolver.is_optional(f) {
        has_optional = true;
    }
    // For opaque method-call accessors (e.g., `result.elements()`), check if the field
    // is a non-Vec type. If so, it would wrap with `.toString()` to convert RustString to Swift String.
    // But if the field is actually a scalar string (not a collection), we cannot meaningfully
    // call .count on it, so return None to signal that this assertion should be skipped.
    let count_target = swift_count_target(&accessor, field_resolver, Some(f))?;
    // `swift_count_target` wraps a scalar-String leaf with `.toString()`, which yields a
    // NON-optional Swift `String`. Appending `?.count` to it is a compile error
    // ("cannot use optional chaining on non-optional value of type 'String'"), so such a
    // target always takes `.count` directly regardless of `has_optional`.
    let target_is_to_string = count_target.ends_with(".toString()");
    Some(if count_target.contains("?.") {
        // An optional ancestor chain already propagated `?`, so `.count` is Optional<Int>;
        // coalesce with `?? 0` to get a concrete Int for XCTAssert.
        format!("({count_target}.count ?? 0)")
    } else if has_optional && !target_is_to_string {
        // The field_expr itself is Optional<RustVec<T>> (no ancestor chain), so unwrap
        // with `?.count` before coalescing.
        format!("({count_target}?.count ?? 0)")
    } else {
        // Non-optional RustVec<T>, or a `.toString()` Swift `String` — `.count` directly.
        format!("{count_target}.count")
    })
}

/// Return the count-able target expression for `field_expr`.
///
/// For opaque method-call accessors (ending in `()` or `()?`), the returned
/// value depends on the field's IR kind:
///
/// - `Vec<T>` ⇒ `RustVec<T>`, which exposes `.count` directly. No wrap.
/// - `String` ⇒ `RustString`, which does NOT expose `.count`; wrap with
///   `.toString()` so the caller's `.count` lands on a Swift `String`, whose
///   character count is the meaningful reading for a scalar.
/// - A JSON-bridged leaf ⇒ `None`. It is a collection or map whose getter is
///   one `RustString`, so neither reading is available: the elements are gone
///   and the character count of the JSON text answers a different question.
///
/// First-class property accessors (no trailing parens) return Swift values
/// that already support `.count` directly.
///
/// The discriminator is the field's resolved leaf type, looked up against the
/// `SwiftFirstClassMap`'s vec field set when available, falling back to the
/// IR-derived `is_array`/`is_collection_root` oracle when the map has no
/// per-field data at all, and deferring to a positive JSON-bridge fact over
/// either when one is recorded. If nothing answers, fall back to wrapping with
/// `.toString()` — the correct treatment for a genuine scalar String field.
///
/// ~keep `leaf_is_vec_via_swift_map`'s own doc already warns it is a bare-leaf,
/// best-effort answer; treating its `false` as "therefore JSON-bridged" — rather
/// than "the map has no opinion" — is what made a field the IR proves is a real
/// `Vec<T>` (but that `SwiftFirstClassMap` never recorded, e.g. reached only
/// through an opaque owner type the map never scanned) fall to `.toString()`
/// and silently count the CHARACTERS of a debug string instead of the Vec's
/// elements. `leaf_is_json_bridged_via_swift_map` is consulted FIRST and wins
/// outright when it fires, because a field the swift-bridge scan positively
/// recorded as JSON-bridged really is a scalar `RustString` at the Swift
/// surface regardless of what the Rust-level IR says its logical shape is —
/// see that method's own doc for why the map cannot answer this from the
/// complement of `vec_field_names` alone.
pub(super) fn swift_count_target(
    field_expr: &str,
    field_resolver: &FieldResolver,
    field: Option<&str>,
) -> Option<String> {
    let is_method_call = field_expr.trim_end().ends_with(')');
    if !is_method_call {
        return Some(field_expr.to_string());
    }
    if let Some(f) = field {
        let resolved = field_resolver.resolve(f);
        // ~keep Every collection shape `field_needs_json_bridge` fires for -- `Option<Vec<T>>`,
        // `Vec<Vec<_>>`, map getters -- reaches Swift as one `RustString` of JSON, so the element
        // count the caller is asking for does not exist on the leaf at all. Returning
        // `Some("{expr}.toString()")` handed the caller the JSON TEXT's character count instead:
        // `count_min` on an empty `Option<Vec<T>>` compared `"[]".count >= 1` and passed, and
        // `not_empty` compared `"null".count > 0` and could not fail. `None` is the honest answer
        // and routes every caller into the `CountOnJsonBridgedLeafInSwift` skip they already
        // spell -- a branch that was unreachable while this returned `Some` on every path.
        if field_resolver.leaf_is_json_bridged_via_swift_map(resolved) {
            return None;
        }
        if field_resolver.leaf_is_vec_via_swift_map(resolved)
            || field_resolver.is_array(resolved)
            || field_resolver.is_collection_root(resolved)
        {
            return Some(field_expr.to_string());
        }
    }
    // A non-Vec method-call accessor is a scalar String (RustString) leaf. Converting
    // it to a Swift `String` via `.toString()` yields a value that DOES expose a
    // meaningful `.count` (character length), so wrap with `.toString()` and let the
    // caller append `.count` for length assertions (e.g. `count_min`, `is_empty`).
    Some(format!("{field_expr}.toString()"))
}

/// `is_empty` predicate for an array field. `field_is_optional` (`Option<Vec<T>>`) is handled
/// by the caller before this is reached; this covers a non-optional array reached through an
/// optional PARENT (`data.children`, `data: Option<Data>`). Swift's earlier `?.` already
/// propagates optionality through the rest of the chain, so `field_expr` needs no extra `?`
/// before `.isEmpty` -- only the `?? true` coalesce (an absent parent counts as vacuously
/// empty), mirroring `swift_count_target`'s scalar `.count ?? 0` fallback. ~keep
pub(super) fn swift_array_is_empty_expr(field_expr: &str, accessor_is_optional: bool) -> String {
    if accessor_is_optional {
        format!("({field_expr}.isEmpty ?? true)")
    } else {
        format!("{field_expr}.isEmpty")
    }
}

/// `not_empty` counterpart to [`swift_array_is_empty_expr`]. `!Bool?` doesn't typecheck, so
/// compare against `false` instead of negating when `accessor_is_optional`. ~keep
pub(super) fn swift_array_not_empty_predicate(field_expr: &str, accessor_is_optional: bool) -> String {
    if accessor_is_optional {
        format!("{field_expr}.isEmpty == false")
    } else {
        format!("!{field_expr}.isEmpty")
    }
}

#[cfg(test)]
mod materialise_vec_temporaries_tests {
    use super::materialise_vec_temporaries;

    /// The confirmed defect: a field-access chain that indexes into a `RustVec` twice
    /// (e.g. `items[0].nested[1].value`, both `items` and `nested` swift-bridge `RustVec`
    /// fields) only had its OUTER `()[...]` hoisted to a local. The inner `.nested()` call
    /// is itself a fresh `RustVec` temporary that was left subscripted inline — the exact
    /// dangling-pointer hazard this function exists to prevent, one level deeper. Both
    /// temporaries must be hoisted, each reading from the previous hoist's local. ~keep
    #[test]
    fn nested_indexed_rust_vec_hoists_every_temporary() {
        let (setup, rewritten, is_map_subscript) =
            materialise_vec_temporaries("result.items()[0].nested()[1].value()", "count_min_ab12").unwrap();

        assert_eq!(
            setup,
            vec![
                "let _vec_items_count_min_ab12_1 = result.items()".to_string(),
                "let _vec_nested_count_min_ab12_2 = _vec_items_count_min_ab12_1[0].nested()".to_string(),
            ]
        );
        assert_eq!(rewritten, "_vec_nested_count_min_ab12_2[1].value()");
        assert!(!is_map_subscript);
    }

    /// Control: a single-level indexed `RustVec` access — the case this function already
    /// handled correctly — must keep hoisting exactly one temporary and leave the tail
    /// after the subscript untouched. ~keep
    #[test]
    fn single_indexed_rust_vec_hoists_one_temporary() {
        let (setup, rewritten, is_map_subscript) =
            materialise_vec_temporaries("result.items()[0].value()", "count_min_ab12").unwrap();

        assert_eq!(
            setup,
            vec!["let _vec_items_count_min_ab12_1 = result.items()".to_string()]
        );
        assert_eq!(rewritten, "_vec_items_count_min_ab12_1[0].value()");
        assert!(!is_map_subscript);
    }

    /// Control: a chain with no `()[...]` subscript at all must be returned unchanged,
    /// with no setup lines emitted. ~keep
    #[test]
    fn non_indexed_chain_is_unchanged() {
        let (setup, rewritten, is_map_subscript) =
            materialise_vec_temporaries("result.items().value()", "count_min_ab12").unwrap();

        assert!(setup.is_empty());
        assert_eq!(rewritten, "result.items().value()");
        assert!(!is_map_subscript);
    }

    /// The confirmed defect: a naive `find(']')` closes a terminal map-key subscript at the
    /// FIRST `]`, which is inside the key itself when the key contains one — `quoted_key_literal`
    /// escapes `\`, `"`, and whitespace control characters, but never `]`. Pre-fix this
    /// misidentified the subscript as `["a]` (unterminated), read `is_string_key` as false (the
    /// trimmed inner text `"a` starts with `"` but does not end with one), and skipped the
    /// JSON-decode setup entirely, emitting the malformed tail `b"]` into the rewritten
    /// expression. ~keep
    #[test]
    fn terminal_map_key_containing_a_bracket_is_recognised_as_a_string_key() {
        let (setup, rewritten, is_map_subscript) =
            materialise_vec_temporaries("result.labels()[\"a]b\"]", "equals_ff01").unwrap();

        assert_eq!(
            setup,
            vec![
                "let _vec_labels_equals_ff01_1 = (try? JSONSerialization.jsonObject(with: \
                 (result.labels().toString() ?? \"{}\").data(using: .utf8)!) as? [String: String]) ?? [:]"
                    .to_string(),
            ]
        );
        assert_eq!(rewritten, "_vec_labels_equals_ff01_1[\"a]b\"]");
        assert!(is_map_subscript);
    }

    /// Pins the documented LAST-subscript semantics against pre-`a531f1441` code, which only
    /// hoisted the FIRST `()[...]` occurrence and would never have reached this expression's
    /// second (map) subscript at all: a chain that indexes a `RustVec` and THEN ends in a
    /// terminal string-key map subscript must report `is_map_subscript == true` (the deepest/
    /// last accessor is the map read). Neither subscript here embeds a bracket in its own
    /// content, so this does NOT exercise the quote-aware bracket scanner
    /// (`find_subscript_close`/`find_next_subscript_marker`) — see
    /// `terminal_map_key_containing_a_bracket_is_recognised_as_a_string_key` above for that, and
    /// `materialise_vec_optional_tests.rs` for the outer-scanner-specific coverage. ~keep
    #[test]
    fn vec_then_terminal_map_subscript_proves_last_subscript_wins() {
        let (setup, rewritten, is_map_subscript) =
            materialise_vec_temporaries("result.items()[0].labels()[\"a\"]", "count_min_9f01").unwrap();

        assert_eq!(
            setup,
            vec![
                "let _vec_items_count_min_9f01_1 = result.items()".to_string(),
                "let _vec_labels_count_min_9f01_2 = (try? JSONSerialization.jsonObject(with: \
                 (_vec_items_count_min_9f01_1[0].labels().toString() ?? \"{}\").data(using: .utf8)!) as? \
                 [String: String]) ?? [:]"
                    .to_string(),
            ]
        );
        assert_eq!(rewritten, "_vec_labels_count_min_9f01_2[\"a\"]");
        assert!(is_map_subscript);
    }

    /// The reachable hazard flagged in review: a string-key (JSON-bridged map) subscript
    /// followed by a FURTHER `RustVec` subscript. Once the map subscript decodes into
    /// `[String: String]`, the value it yields is a plain Swift `String` — a further `()[`
    /// hoist against it would compile against the wrong type. `json_bridged_traversal_skip`
    /// (leaf_shape.rs) refuses this shape earlier when IR data positively classified the map
    /// field as JSON-bridged, but a config-only/opaque resolver (no IR wired in) never does, so
    /// this function must refuse it directly rather than emit broken Swift. ~keep
    #[test]
    fn mixed_map_then_vec_subscript_is_refused() {
        let result = materialise_vec_temporaries("result.labels()[\"a\"].items()[0]", "not_empty_77aa");

        assert!(result.is_none(), "got: {result:?}");
    }
}

#[cfg(test)]
mod nested_wildcard_tests {
    use super::{SwiftTraversalContains, swift_traversal_contains_assert};
    use crate::e2e::field_access::FieldResolver;
    use std::collections::{HashMap, HashSet};

    fn array_resolver(field: &str) -> FieldResolver {
        let names: HashSet<String> = [field.to_string()].into_iter().collect();
        FieldResolver::new(&HashMap::new(), &HashSet::new(), &names, &names, &HashSet::new())
    }

    fn render(full_field: &str, array_part: &str, element_part: &str, resolver: &FieldResolver) -> String {
        swift_traversal_contains_assert(SwiftTraversalContains {
            array_part,
            element_part,
            full_field,
            value_expression: "\"example.test\"",
            result_variable: "result",
            negate: false,
            message: "expected to contain",
            field_resolver: resolver,
        })
    }

    /// Baseline: a single wildcard still builds a `contains(where:)` over the whole array, so
    /// the refusal below cannot have been implemented by refusing traversals generally. ~keep
    #[test]
    fn single_wildcard_still_builds_a_contains_where_closure() {
        let line = render("links[].url", "links", "url", &array_resolver("links"));
        assert!(line.contains(".contains(where: {"), "got: {line}");
        assert!(!line.contains("skipped:"), "got: {line}");
    }

    /// The element sub-path handed to this helper keeps everything after the FIRST `[].`, so a
    /// doubly-nested path arrives here as `links[].url` and `accessor` lowers the surviving
    /// wildcard to index 0 — the closure would range over `pages` while reading `links[0]`.
    /// Pre-guard this test fails: the returned line is a `contains(where:)` naming `[0]`. ~keep
    #[test]
    fn nested_wildcard_should_return_a_visible_skip_rather_than_an_index_zero_check() {
        let line = render("pages[].links[].url", "pages", "links[].url", &array_resolver("pages"));
        assert_eq!(
            line, "        // skipped: nested array-wildcard field 'pages[].links[].url' not supported",
            "got: {line}"
        );
    }
}
