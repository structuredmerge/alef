use crate::core::config::Language;
use crate::core::ir::{ApiSurface, DefaultValue, FieldDef, PrimitiveType, TypeRef};
use crate::docs::enum_variant_ref::format_enum_variant_ref;
use crate::docs::type_mapping::doc_type;
use crate::docs::type_mapping::java_boxed_type;
use heck::ToPascalCase;

/// Escape literal pipe characters and square brackets outside code spans so a value can be
/// embedded inside a Markdown table cell without being misinterpreted.
///
/// This is required because some renderers (notably rumdl's MD056 check) treat
/// every `|` on a row as a separator, even when the pipe is inside a backtick
/// span — e.g. union types like `` `string | null` `` or `` `String.t() | nil` ``
/// would otherwise be parsed as two cells.
///
/// Square brackets inside backticks (e.g., `` `list[Table]` ``) are parsed as
/// link references by Zensical's CommonMark parser, causing "unresolved reference"
/// warnings in strict mode. We escape them to prevent this.
pub(crate) fn escape_table_cell(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    let mut code_delimiter = 0;
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '`' {
            let mut run = 1;
            while chars.peek() == Some(&'`') {
                chars.next();
                run += 1;
            }
            if code_delimiter == 0 {
                code_delimiter = run;
            } else if code_delimiter == run {
                code_delimiter = 0;
            }
            escaped.extend(std::iter::repeat_n('`', run));
        } else if ch == '|' {
            escaped.push_str("\\|");
        } else if code_delimiter == 0 && matches!(ch, '[' | ']') {
            escaped.push('\\');
            escaped.push(ch);
        } else {
            escaped.push(ch);
        }
    }

    escaped
}

#[cfg(test)]
mod table_escape_tests {
    use super::escape_table_cell;

    #[test]
    fn preserves_brackets_in_code_spans_and_escapes_table_syntax() {
        assert_eq!(
            escape_table_cell("Use `[Item]` or [Item] | none"),
            "Use `[Item]` or \\[Item\\] \\| none"
        );
    }

    #[test]
    fn preserves_brackets_in_multi_backtick_code_spans() {
        assert_eq!(escape_table_cell("``value[`key`]``"), "``value[`key`]``");
    }
}

pub(crate) fn format_field_default(field: &FieldDef, lang: Language, api: &ApiSurface, ffi_prefix: &str) -> String {
    if let Some(typed) = &field.typed_default {
        return format_typed_default(typed, &field.ty, lang, api, ffi_prefix, field.optional);
    }
    if let Some(raw) = &field.default
        && !raw.is_empty()
    {
        let collapsed = crate::docs::doc_cleaning::collapse_whitespace(raw);
        return format!("`{collapsed}`");
    }
    if field.optional {
        return match lang {
            Language::Python => "`None`".to_string(),
            Language::Node | Language::Wasm => "`null`".to_string(),
            Language::Go => "`nil`".to_string(),
            Language::Java => "`null`".to_string(),
            Language::Csharp => "`null`".to_string(),
            Language::Ruby => "`nil`".to_string(),
            Language::Php => "`null`".to_string(),
            Language::Elixir => "`nil`".to_string(),
            Language::R => "`NULL`".to_string(),
            Language::Rust => "`None`".to_string(),
            Language::Ffi | Language::C | Language::Jni => "`NULL`".to_string(),
            Language::Kotlin
            | Language::KotlinAndroid
            | Language::Swift
            | Language::Dart
            | Language::Gleam
            | Language::Zig => "`null`".to_string(),
        };
    }
    "—".to_string()
}

/// Qualifies a bare variant name (`Curated`) with its owning enum type when `field_ty` names
/// one, mirroring the fallback branch of the `DefaultValue::EnumVariant` case below — shared so
/// `TupleVariant` and `StructVariant` document a variant reference the same way a unit variant
/// does. Unlike `EnumVariant`, `TupleVariant`/`StructVariant` never carry a `Type::Variant`
/// qualified string, so there is no `splitn` case to mirror. ~keep
fn enum_variant_doc_label(variant: &str, field_ty: &TypeRef, lang: Language, ffi_prefix: &str) -> String {
    let enum_type_name_str = match field_ty {
        TypeRef::Named(n) => Some(n.as_str()),
        TypeRef::Optional(inner) => match inner.as_ref() {
            TypeRef::Named(n) => Some(n.as_str()),
            _ => None,
        },
        _ => None,
    };
    match enum_type_name_str {
        Some(type_str) => format_enum_variant_ref(type_str, variant, lang, ffi_prefix),
        None => variant.to_string(),
    }
}

pub(crate) fn format_typed_default(
    val: &DefaultValue,
    field_ty: &TypeRef,
    lang: Language,
    api: &ApiSurface,
    ffi_prefix: &str,
    optional: bool,
) -> String {
    match val {
        DefaultValue::BoolLiteral(b) => match lang {
            Language::Python => format!("`{}`", if *b { "True" } else { "False" }),
            _ => format!("`{b}`"),
        },
        DefaultValue::StringLiteral(s) => {
            let normalized = crate::docs::doc_cleaning::collapse_whitespace(s);
            format!("`\"{normalized}\"`")
        }
        // Documented in a language-neutral bracket form. Elements route back through this same
        // function so a Python `True` stays `True` inside the list; the per-element backticks
        // are stripped because the whole list carries one pair. ~keep
        DefaultValue::ListLiteral(items) => {
            let rendered: Vec<String> = items
                .iter()
                .map(|item| {
                    format_typed_default(item, field_ty, lang, api, ffi_prefix, optional)
                        .trim_matches('`')
                        .to_string()
                })
                .collect();
            format!("`[{}]`", rendered.join(", "))
        }
        DefaultValue::IntLiteral(n) => {
            if matches!(field_ty, TypeRef::Duration)
                || matches!(field_ty, TypeRef::Optional(inner) if matches!(inner.as_ref(), TypeRef::Duration))
            {
                return format!("`{n}ms`");
            }
            format!("`{n}`")
        }
        DefaultValue::FloatLiteral(f) => format!("`{f}`"),
        DefaultValue::EnumVariant(v) => {
            let parts: Vec<&str> = v.splitn(2, "::").collect();
            if parts.len() == 2 {
                format!("`{}`", format_enum_variant_ref(parts[0], parts[1], lang, ffi_prefix))
            } else {
                let enum_type_name_str = match field_ty {
                    TypeRef::Named(n) => Some(n.as_str()),
                    TypeRef::Optional(inner) => {
                        if let TypeRef::Named(n) = inner.as_ref() {
                            Some(n.as_str())
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                if let Some(type_str) = enum_type_name_str {
                    format!("`{}`", format_enum_variant_ref(type_str, v, lang, ffi_prefix))
                } else {
                    format!("`{v}`")
                }
            }
        }
        // A tuple- or struct-variant default carries its own values, so — unlike the bare
        // `EnumVariant` case — there is always something concrete to document; each argument or
        // field routes back through this same function, the way `ListLiteral`'s elements do. ~keep
        DefaultValue::TupleVariant(variant, values) => {
            let rendered: Vec<String> = values
                .iter()
                .map(|item| {
                    format_typed_default(item, field_ty, lang, api, ffi_prefix, optional)
                        .trim_matches('`')
                        .to_string()
                })
                .collect();
            format!(
                "`{}({})`",
                enum_variant_doc_label(variant, field_ty, lang, ffi_prefix),
                rendered.join(", ")
            )
        }
        DefaultValue::StructVariant(variant, fields) => {
            let rendered: Vec<String> = fields
                .iter()
                .map(|(name, item)| {
                    let value = format_typed_default(item, field_ty, lang, api, ffi_prefix, optional)
                        .trim_matches('`')
                        .to_string();
                    format!("{name}: {value}")
                })
                .collect();
            format!(
                "`{} {{ {} }}`",
                enum_variant_doc_label(variant, field_ty, lang, ffi_prefix),
                rendered.join(", ")
            )
        }
        // `Unresolved` documents the type-zero, same as `Empty`. That is the prose half of the
        // very drift this variant exists to expose — generated docs claiming `0ms` for a default
        // alef could not read — and it is left here only because the validation pass refuses the
        // surface before docs are written unless the crate suppressed the diagnostic. ~keep
        DefaultValue::Empty | DefaultValue::Unresolved(_) => {
            let inner_for_dur = match field_ty {
                TypeRef::Optional(inner) => Some(inner.as_ref()),
                other => Some(other),
            };
            if matches!(inner_for_dur, Some(TypeRef::Duration)) {
                return match lang {
                    Language::Rust => "`Duration::default()`".to_string(),
                    _ => "`0ms`".to_string(),
                };
            }

            if !optional
                && let TypeRef::Named(type_name_str) = field_ty
                && let Some(enum_def) = api.enums.iter().find(|e| &e.name == type_name_str)
            {
                let variant = enum_def
                    .variants
                    .iter()
                    .find(|v| v.is_default)
                    .or_else(|| enum_def.variants.first());
                if let Some(v) = variant {
                    return format!(
                        "`{}`",
                        format_enum_variant_ref(type_name_str, &v.name, lang, ffi_prefix)
                    );
                }
            }
            let inner_ty = match field_ty {
                TypeRef::Optional(inner) => inner.as_ref(),
                other => other,
            };
            if matches!(inner_ty, TypeRef::Vec(_)) {
                return match lang {
                    Language::Python => "`[]`".to_string(),
                    Language::Node | Language::Wasm => "`[]`".to_string(),
                    Language::Go => "`nil`".to_string(),
                    Language::Java => "`Collections.emptyList()`".to_string(),
                    Language::Csharp => {
                        let elem_ty = if let TypeRef::Vec(elem) = inner_ty {
                            doc_type(elem, lang, ffi_prefix)
                        } else {
                            String::new()
                        };
                        format!("`new List<{elem_ty}>()`")
                    }
                    Language::Ruby | Language::Elixir => "`[]`".to_string(),
                    Language::Php => "`[]`".to_string(),
                    Language::Rust => "`vec![]`".to_string(),
                    Language::Ffi | Language::C | Language::Jni => "`NULL`".to_string(),
                    Language::R => "`list()`".to_string(),
                    Language::Kotlin
                    | Language::KotlinAndroid
                    | Language::Swift
                    | Language::Dart
                    | Language::Gleam
                    | Language::Zig => "`[]`".to_string(),
                };
            }
            if matches!(inner_ty, TypeRef::Map(_, _)) {
                return match lang {
                    Language::Python | Language::Ruby | Language::Php => "`{}`".to_string(),
                    Language::Node | Language::Wasm => "`{}`".to_string(),
                    Language::Go => "`nil`".to_string(),
                    Language::Elixir => "`%{}`".to_string(),
                    Language::Java => "`Collections.emptyMap()`".to_string(),
                    Language::Csharp => {
                        if let TypeRef::Map(k, v) = inner_ty {
                            let kty = doc_type(k, lang, ffi_prefix);
                            let vty = doc_type(v, lang, ffi_prefix);
                            format!("`new Dictionary<{kty}, {vty}>()`")
                        } else {
                            "`new Dictionary<>()`".to_string()
                        }
                    }
                    Language::Rust => "`HashMap::new()`".to_string(),
                    Language::Ffi | Language::C | Language::Jni => "`NULL`".to_string(),
                    Language::R => "`list()`".to_string(),
                    Language::Kotlin
                    | Language::KotlinAndroid
                    | Language::Swift
                    | Language::Dart
                    | Language::Gleam
                    | Language::Zig => "`{}`".to_string(),
                };
            }
            if !optional {
                return "—".to_string();
            }
            match lang {
                Language::Python => "`None`".to_string(),
                Language::Node | Language::Wasm => "`null`".to_string(),
                Language::Go => "`nil`".to_string(),
                Language::Java => "`null`".to_string(),
                Language::Csharp => "`null`".to_string(),
                Language::Ruby => "`nil`".to_string(),
                Language::Php => "`null`".to_string(),
                Language::Elixir => "`nil`".to_string(),
                Language::R => "`NULL`".to_string(),
                Language::Rust => "`Default::default()`".to_string(),
                Language::Ffi | Language::C | Language::Jni => "`NULL`".to_string(),
                Language::Kotlin
                | Language::KotlinAndroid
                | Language::Swift
                | Language::Dart
                | Language::Gleam
                | Language::Zig => "`null`".to_string(),
            }
        }
        DefaultValue::None => {
            if !optional {
                return "—".to_string();
            }
            match lang {
                Language::Python => "`None`".to_string(),
                Language::Node | Language::Wasm => "`null`".to_string(),
                Language::Go => "`nil`".to_string(),
                Language::Java => "`null`".to_string(),
                Language::Csharp => "`null`".to_string(),
                Language::Ruby => "`nil`".to_string(),
                Language::Php => "`null`".to_string(),
                Language::Elixir => "`nil`".to_string(),
                Language::R => "`NULL`".to_string(),
                Language::Rust => "`None`".to_string(),
                Language::Ffi | Language::C | Language::Jni => "`NULL`".to_string(),
                Language::Kotlin
                | Language::KotlinAndroid
                | Language::Swift
                | Language::Dart
                | Language::Gleam
                | Language::Zig => "`null`".to_string(),
            }
        }
        DefaultValue::FunctionCall(path) | DefaultValue::PublicFunctionCall(path) => format!("`{path}()`"),
    }
}

/// ~keep The C ABI's on-error return value depends on the function's return shape, not a
/// single fixed sentinel -- traced from `backends/ffi/gen_bindings/functions/orchestration.rs`
/// (`:223-229`/`:760-765`, `gen_function_wrapper_footer` `:615-617`) and `helpers.rs`
/// (`:193-232`): a fallible `()` return, or any `Bytes` result (always delivered via
/// out-param + status), reports failure as `-1`; a `TypeRef::Named` handle reports it as
/// the integer `0` -- never `NULL`, there is no pointer to compare a `uint64_t` handle
/// against, which is exactly the confusion this fix exists to remove; the remaining
/// heap-allocated shapes (String/Char/Path/Json/Vec/Map) really do return a null pointer;
/// numeric primitives and `Duration` report `0` (`0.0` for floats). Must stay in lockstep
/// with the `int32_t`/`-1` text in `function_render.rs`'s `push_returns_with_override` --
/// see `ffi_status_code.rs`'s cross-check tests for why the two must not drift apart.
fn ffi_error_return_phrase(return_type: &TypeRef) -> String {
    match return_type {
        TypeRef::Unit | TypeRef::Bytes => "Returns `-1` on error.".to_string(),
        TypeRef::Named(_) => "Returns the sentinel handle `0` on error.".to_string(),
        TypeRef::String | TypeRef::Char | TypeRef::Path | TypeRef::Json | TypeRef::Vec(_) | TypeRef::Map(_, _) => {
            "Returns `NULL` on error.".to_string()
        }
        TypeRef::Primitive(PrimitiveType::F32 | PrimitiveType::F64) => "Returns `0.0` on error.".to_string(),
        TypeRef::Primitive(_) | TypeRef::Duration => "Returns `0` on error.".to_string(),
        TypeRef::Optional(inner) => ffi_error_return_phrase(inner),
    }
}

/// Format the error/exception phrase for a function that can fail.
pub(crate) fn format_error_phrase(error_type: &str, return_type: &TypeRef, lang: Language, crate_name: &str) -> String {
    let short = error_type.rsplit("::").next().unwrap_or(error_type);
    match lang {
        Language::Python => {
            let ename = short.to_pascal_case();
            format!("Raises `{ename}`.")
        }
        Language::Go => "Returns `error`.".to_string(),
        // ~keep The class is named by `backends::java::naming::exception_class_name`, the same
        // derivation the signature's `throws` clause (`signatures.rs`) and the streaming
        // override (`streaming.rs`) use. Pascal-casing the error type's own short name here
        // used to name a *different* class -- one the domain-error hierarchy happens to
        // declare (`codegen/error_gen/host_langs.rs`) but that no method actually throws.
        Language::Java => {
            let ename = crate::backends::java::naming::exception_class_name(crate_name);
            format!("Throws `{ename}`.")
        }
        Language::Node | Language::Wasm => "Throws `Error` with a descriptive message.".to_string(),
        Language::Ruby => {
            let ename = short.to_pascal_case();
            format!("Raises `{ename}`.")
        }
        Language::Csharp => {
            let ename = short.to_pascal_case();
            format!("Throws `{ename}`.")
        }
        Language::Elixir => "Returns `{:error, reason}`".to_string(),
        Language::Php => {
            let ename = short.to_pascal_case();
            format!("Throws `{ename}`.")
        }
        Language::Ffi | Language::C => ffi_error_return_phrase(return_type),
        // ~keep Jni is reachable (see docs::examples::sample_param_value) and this fixed `NULL`
        // phrase is wrong for it: a JNI shim signals failure by throwing
        // `<Bridge>Exception` via `env.throw_new`, then returns a type-specific zero — `()`,
        // `false`, `0`, or `std::ptr::null_mut()` (backends/jni/gen_shims/type_helpers.rs:29-42,
        // templates/runtime_helpers.rs.jinja:32-42). "Returns `NULL`" is wrong for unit,
        // primitive, and handle returns alike, and buries the exception the caller must catch.
        Language::Jni => "Returns `NULL` on error.".to_string(),
        Language::R => "Stops with error message.".to_string(),
        Language::Rust => {
            let ename = short.to_pascal_case();
            format!("Returns `Err({ename})`.")
        }
        Language::Kotlin
        | Language::KotlinAndroid
        | Language::Swift
        | Language::Dart
        | Language::Gleam
        | Language::Zig => {
            let ename = short.to_pascal_case();
            format!("Throws `{ename}`.")
        }
    }
}

/// Like `doc_type` but wraps in the nullable form when `optional` is true.
pub(crate) fn doc_type_with_optional(ty: &TypeRef, lang: Language, optional: bool, ffi_prefix: &str) -> String {
    if optional && !matches!(ty, TypeRef::Optional(_)) {
        let inner = doc_type(ty, lang, ffi_prefix);
        return match lang {
            Language::Python => format!("{inner} | None"),
            Language::Node | Language::Wasm => format!("{inner} | null"),
            Language::Go => format!("*{inner}"),
            // ~keep The public facade unwraps the internal `...Rs` `Optional<T>` and returns
            // `@Nullable T` instead, calling `render_nullable_type` for both parameters and
            // returns (backends/java/gen_bindings/facade.rs:37,62). Delegating here, the same
            // way type_mapping.rs's `TypeRef::Optional` arm does, is what keeps this copy from
            // drifting off the facade; matching its output by hand is what produced the drift
            // this arm fixes. Record fields land on the same `@Nullable T` shape but do NOT
            // route through `render_nullable_type` -- gen_bindings/types/records.rs:87-155
            // hand-rolls the annotation from `has_nullable`/`nullable_at_leading_pos` -- so
            // that third copy remains free to drift and is not guarded by this delegation.
            Language::Java => {
                let boxed = java_boxed_type(ty);
                crate::backends::java::gen_bindings::helpers::render_nullable_type(&boxed, true)
            }
            Language::Csharp => format!("{inner}?"),
            Language::Ruby => format!("{inner}?"),
            Language::Php => format!("?{inner}"),
            Language::Elixir => format!("{inner} | nil"),
            Language::R => format!("{inner} or NULL"),
            Language::Rust => format!("Option<{inner}>"),
            // ~keep Mirrors type_mapping.rs's `TypeRef::Optional` arm: a Named type crosses
            // the C ABI as a scalar `AlefHandle`, never a pointer, so wrapping it in another
            // `*` here fabricates a pointer that doesn't exist. Types that already render as
            // pointers (strings, bytes, JSON, maps) are nullable in place -- doubling the `*`
            // for those was the `const char**`-vs-header `const char *` divergence. Jni falls
            // into the pointer arm below, which is wrong for it: the JNI backend emits Rust,
            // where an optional is `jlong`/`jbyteArray`/`jstring`, never a pointer. See
            // docs::examples::sample_param_value.
            Language::Ffi | Language::C if matches!(ty, TypeRef::Named(_)) || inner.ends_with('*') => inner,
            Language::Ffi | Language::C | Language::Jni => format!("{inner}*"),
            Language::Kotlin
            | Language::KotlinAndroid
            | Language::Swift
            | Language::Dart
            | Language::Gleam
            | Language::Zig => {
                format!("{inner}?")
            }
        };
    }
    doc_type(ty, lang, ffi_prefix)
}

/// ~keep Every documented identifier (method/function/type name) must be one the target
/// language's own toolchain would actually accept. A curated or templated name that turns
/// out to collide with a reserved word (Java's `new`, Dart's `new`, ...) fails silently in
/// the worst way: the docs render, look entirely plausible, and only a `javac`/`dart
/// analyze` run against the *real* generated code catches it -- see the tslp `new`
/// vs `create` finding this gate was built to close. Unlike a per-language name fix, this
/// keeps working when whatever mechanism produces a name (a template, a curated override,
/// config) changes to something new that happens to collide again -- it makes the whole
/// class of "wrong reserved-word identifier" impossible to emit silently, not just today's
/// instance of it. Keyword lists are the well-known reserved-word set for each language;
/// where a language's contextual/soft-keyword rules are large (Kotlin, Swift, C#), the
/// list is a conservative subset that includes every word reserved in *every* context, not
/// an exhaustive grammar -- see each arm's word count for how conservative.
///
/// These lists answer "is this word reserved", not "is it usable here": the grammatical
/// position a name occupies is a separate axis, applied by
/// `identifier_violation`/`reserved_words_bind_in_member_position`. Do not prune a word
/// from a list because some position accepts it.
fn reserved_words(lang: Language) -> &'static [&'static str] {
    match lang {
        Language::Python => &[
            "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def",
            "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
            "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield",
        ],
        Language::Node | Language::Wasm => &[
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "debugger",
            "default",
            "delete",
            "do",
            "else",
            "export",
            "extends",
            "false",
            "finally",
            "for",
            "function",
            "if",
            "import",
            "in",
            "instanceof",
            "new",
            "null",
            "return",
            "super",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "typeof",
            "var",
            "void",
            "while",
            "with",
            "yield",
            "let",
            "static",
            "enum",
            "await",
            "implements",
            "package",
            "protected",
            "interface",
            "private",
            "public",
        ],
        Language::Ruby => &[
            "__ENCODING__",
            "__LINE__",
            "__FILE__",
            "BEGIN",
            "END",
            "alias",
            "and",
            "begin",
            "break",
            "case",
            "class",
            "def",
            "defined?",
            "do",
            "else",
            "elsif",
            "end",
            "ensure",
            "false",
            "for",
            "if",
            "in",
            "module",
            "next",
            "nil",
            "not",
            "or",
            "redo",
            "rescue",
            "retry",
            "return",
            "self",
            "super",
            "then",
            "true",
            "undef",
            "unless",
            "until",
            "when",
            "while",
            "yield",
        ],
        Language::Php => &[
            "abstract",
            "and",
            "array",
            "as",
            "break",
            "callable",
            "case",
            "catch",
            "class",
            "clone",
            "const",
            "continue",
            "declare",
            "default",
            "do",
            "echo",
            "else",
            "elseif",
            "empty",
            "enddeclare",
            "endfor",
            "endforeach",
            "endif",
            "endswitch",
            "endwhile",
            "extends",
            "final",
            "finally",
            "fn",
            "for",
            "foreach",
            "function",
            "global",
            "goto",
            "if",
            "implements",
            "include",
            "include_once",
            "instanceof",
            "insteadof",
            "interface",
            "isset",
            "list",
            "match",
            "namespace",
            "new",
            "or",
            "print",
            "private",
            "protected",
            "public",
            "readonly",
            "require",
            "require_once",
            "return",
            "static",
            "switch",
            "throw",
            "trait",
            "try",
            "unset",
            "use",
            "var",
            "while",
            "xor",
            "yield",
        ],
        Language::Elixir => &[
            "true", "false", "nil", "when", "and", "or", "not", "in", "fn", "do", "end", "catch", "rescue", "after",
            "else",
        ],
        Language::Go => &[
            "break",
            "default",
            "func",
            "interface",
            "select",
            "case",
            "defer",
            "go",
            "map",
            "struct",
            "chan",
            "else",
            "goto",
            "package",
            "switch",
            "const",
            "fallthrough",
            "if",
            "range",
            "type",
            "continue",
            "for",
            "import",
            "return",
            "var",
        ],
        // ~keep Delegates to `core::keywords::JAVA_KEYWORDS` rather than carrying a second copy
        // of the same list: the two used to disagree (this table included the reserved
        // *literals* `true`/`false`/`null`, `JAVA_KEYWORDS` did not), which is exactly the
        // "identifier gate catches it, but the code that would trigger the gate never runs
        // through the gate's own table" bug class this module's own docs warn about. Java has
        // no contextual/soft keywords beyond the JLS's fixed reserved-word set, so unlike
        // Kotlin/Swift/C#/Dart below, there is no broader position-aware superset this table
        // needs that the escaping-side table doesn't also need.
        Language::Java => crate::core::keywords::JAVA_KEYWORDS,
        Language::Csharp => &[
            "abstract",
            "as",
            "base",
            "bool",
            "break",
            "byte",
            "case",
            "catch",
            "char",
            "checked",
            "class",
            "const",
            "continue",
            "decimal",
            "default",
            "delegate",
            "do",
            "double",
            "else",
            "enum",
            "event",
            "explicit",
            "extern",
            "false",
            "finally",
            "fixed",
            "float",
            "for",
            "foreach",
            "goto",
            "if",
            "implicit",
            "in",
            "int",
            "interface",
            "internal",
            "is",
            "lock",
            "long",
            "namespace",
            "new",
            "null",
            "object",
            "operator",
            "out",
            "override",
            "params",
            "private",
            "protected",
            "public",
            "readonly",
            "ref",
            "return",
            "sbyte",
            "sealed",
            "short",
            "sizeof",
            "stackalloc",
            "static",
            "string",
            "struct",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "typeof",
            "uint",
            "ulong",
            "unchecked",
            "unsafe",
            "ushort",
            "using",
            "virtual",
            "void",
            "volatile",
            "while",
        ],
        Language::Rust => &[
            "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for", "if", "impl",
            "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static",
            "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "async", "await", "dyn",
            "abstract", "become", "box", "do", "final", "macro", "override", "priv", "try", "typeof", "unsized",
            "virtual", "yield",
        ],
        Language::Kotlin | Language::KotlinAndroid => &[
            "as",
            "break",
            "class",
            "continue",
            "do",
            "else",
            "false",
            "for",
            "fun",
            "if",
            "in",
            "interface",
            "is",
            "null",
            "object",
            "package",
            "return",
            "super",
            "this",
            "throw",
            "true",
            "try",
            "typealias",
            "typeof",
            "val",
            "var",
            "when",
            "while",
            "by",
            "catch",
            "constructor",
            "delegate",
            "dynamic",
            "field",
            "file",
            "finally",
            "get",
            "import",
            "init",
            "param",
            "property",
            "receiver",
            "set",
            "setparam",
            "where",
            "actual",
            "abstract",
            "annotation",
            "companion",
            "const",
            "crossinline",
            "data",
            "enum",
            "expect",
            "external",
            "final",
            "infix",
            "inline",
            "inner",
            "internal",
            "lateinit",
            "noinline",
            "open",
            "operator",
            "out",
            "override",
            "private",
            "protected",
            "public",
            "reified",
            "sealed",
            "suspend",
            "tailrec",
            "vararg",
        ],
        Language::Swift => &[
            "associatedtype",
            "class",
            "deinit",
            "enum",
            "extension",
            "fileprivate",
            "func",
            "import",
            "init",
            "inout",
            "internal",
            "let",
            "open",
            "operator",
            "private",
            "protocol",
            "public",
            "rethrows",
            "static",
            "struct",
            "subscript",
            "typealias",
            "var",
            "break",
            "case",
            "continue",
            "default",
            "defer",
            "do",
            "else",
            "fallthrough",
            "for",
            "guard",
            "if",
            "in",
            "repeat",
            "return",
            "switch",
            "where",
            "while",
            "as",
            "Any",
            "catch",
            "false",
            "is",
            "nil",
            "self",
            "Self",
            "super",
            "throw",
            "throws",
            "true",
            "try",
        ],
        Language::Dart => &[
            "abstract",
            "as",
            "assert",
            "async",
            "await",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "covariant",
            "default",
            "deferred",
            "do",
            "dynamic",
            "else",
            "enum",
            "export",
            "extends",
            "extension",
            "external",
            "factory",
            "false",
            "final",
            "finally",
            "for",
            "Function",
            "get",
            "hide",
            "if",
            "implements",
            "import",
            "in",
            "interface",
            "is",
            "late",
            "library",
            "mixin",
            "new",
            "null",
            "on",
            "operator",
            "part",
            "required",
            "rethrow",
            "return",
            "set",
            "show",
            "static",
            "super",
            "switch",
            "sync",
            "this",
            "throw",
            "true",
            "try",
            "typedef",
            "var",
            "void",
            "while",
            "with",
            "yield",
        ],
        Language::Gleam => &[
            "as", "assert", "case", "const", "external", "fn", "if", "import", "let", "opaque", "pub", "todo", "type",
            "use", "panic", "echo",
        ],
        Language::Zig => &[
            "addrspace",
            "align",
            "allowzero",
            "and",
            "anyframe",
            "anytype",
            "asm",
            "async",
            "await",
            "break",
            "callconv",
            "catch",
            "comptime",
            "const",
            "continue",
            "defer",
            "else",
            "enum",
            "errdefer",
            "error",
            "export",
            "extern",
            "fn",
            "for",
            "if",
            "inline",
            "noalias",
            "noinline",
            "nosuspend",
            "opaque",
            "or",
            "orelse",
            "packed",
            "pub",
            "resume",
            "return",
            "linksection",
            "struct",
            "suspend",
            "switch",
            "test",
            "threadlocal",
            "try",
            "union",
            "unreachable",
            "usingnamespace",
            "var",
            "volatile",
            "while",
        ],
        Language::R => &[
            "if",
            "else",
            "repeat",
            "while",
            "function",
            "for",
            "in",
            "next",
            "break",
            "TRUE",
            "FALSE",
            "NULL",
            "Inf",
            "NaN",
            "NA",
            "NA_integer_",
            "NA_real_",
            "NA_character_",
            "NA_complex_",
        ],
        Language::Ffi | Language::C | Language::Jni => &[
            "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else", "enum", "extern",
            "float", "for", "goto", "if", "inline", "int", "long", "register", "restrict", "return", "short", "signed",
            "sizeof", "static", "struct", "switch", "typedef", "union", "unsigned", "void", "volatile", "while",
        ],
    }
}

/// A conservative "is this well-formed" check: alphabetic-or-underscore start, then
/// alphanumeric-or-underscore. This is a defensive backstop, not the primary rule --
/// every name this pipeline emits is derived from a Rust identifier via heck's case
/// converters, which never introduce illegal characters, so in practice only the
/// reserved-word check below ever fires.
fn has_valid_identifier_syntax(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Where in the target language's grammar a generated name lands.
///
/// A reserved word is only unusable in the slot a grammar treats as a *bare* identifier;
/// several of these languages admit keywords once the name is a member. The docs pipeline
/// emits names into both kinds of slot from the same naming helpers, so a gate that cannot
/// tell them apart has to guess -- and guessing "bare" is what made it reject
/// `static new(version: string): DownloadManager`, a member declaration the napi backend
/// really does emit into `index.d.ts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdentifierPosition {
    /// A bare binding name: a top-level `function`/`def`/`fn` declaration, a type or class
    /// heading, a local, a parameter. Every language's reserved words are illegal here.
    Declaration,
    /// A member name: a class or struct method/property declaration, or the name following
    /// a `.` in a member access.
    Member,
}

impl IdentifierPosition {
    /// The word used for this position in diagnostics.
    fn label(self) -> &'static str {
        match self {
            Self::Declaration => "declaration",
            Self::Member => "member",
        }
    }
}

/// Whether `lang`'s reserved words stay unusable when the name occupies member position.
///
/// True for every language except the two whose grammars explicitly free the member slot:
///
/// - **TypeScript / JavaScript** (`Node`, `Wasm`). ES5 dropped ES3's restriction on
///   reserved words after `.` and in object-literal keys; ECMA-262 defines a class
///   element's `ClassElementName` as a `PropertyName`, and `PropertyName` accepts any
///   `IdentifierName` -- keywords included. So `class A { static new() {} }` and `a.new`
///   both parse, and TypeScript inherits the rule. The napi backend depends on it:
///   `to_node_name` (codegen/naming.rs) lower-camel-cases a method name with no keyword
///   escape at all, and the `index.d.ts` it produces really does declare
///   `static new(version: string): DownloadManager`.
/// - **PHP**. The PHP 7.0 "Context Sensitive Lexer" RFC made every reserved word usable as
///   a method name, property name and class-constant name. The PHP backend depends on it
///   too -- its method emitters use a bare `method.name.to_lower_camel_case()` with no
///   keyword escape; the one escape that survives, `escape_php_reserved_constant`
///   (php/gen_bindings/types/enums.rs), covers the class-constant position the RFC left
///   partly reserved, not methods.
///
/// Dart deliberately stays on the strict side. `new` is in Dart's `RESERVED_WORD` set, and
/// a Dart member declaration takes an `identifier`, which excludes reserved words outright
/// -- unlike Dart's separate built-in-identifier set (`get`, `factory`, `library`, ...),
/// which *is* usable as an ordinary name. So `new` is illegal for Dart in either position
/// and the gate must keep saying so.
fn reserved_words_bind_in_member_position(lang: Language) -> bool {
    !matches!(lang, Language::Node | Language::Wasm | Language::Php)
}

/// Returns the reason `name` cannot legally appear at `position` in `lang`, or `None` if it
/// can.
///
/// Syntactic well-formedness is checked in both positions -- no language here accepts an
/// unquoted `1invalid` as a member name any more than as a declaration -- while the
/// reserved-word rule is relaxed exactly where `reserved_words_bind_in_member_position`
/// says the grammar relaxes it.
pub(crate) fn identifier_violation(name: &str, lang: Language, position: IdentifierPosition) -> Option<&'static str> {
    if name.is_empty() {
        return Some("empty identifier");
    }
    let reserved_applies = position == IdentifierPosition::Declaration || reserved_words_bind_in_member_position(lang);
    if reserved_applies && reserved_words(lang).contains(&name) {
        return Some("reserved word");
    }
    if !has_valid_identifier_syntax(name) {
        return Some("not a syntactically valid identifier");
    }
    None
}

/// A generated name the docs pipeline must not emit, and everything needed to find it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IdentifierViolation {
    pub(crate) name: String,
    pub(crate) lang: Language,
    pub(crate) position: IdentifierPosition,
    pub(crate) reason: &'static str,
    pub(crate) context: String,
}

impl std::fmt::Display for IdentifierViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            name,
            lang,
            position,
            reason,
            context,
        } = self;
        write!(
            f,
            "generated {lang:?} identifier `{name}` is invalid ({reason}) in {} position while rendering {context} \
             -- the docs pipeline must not emit an identifier a {lang:?} toolchain would reject",
            position.label()
        )
    }
}

impl std::error::Error for IdentifierViolation {}

/// The fallible form of the identifier gate: the structured error a caller can inspect,
/// propagate, or render.
pub(crate) fn check_identifier(
    name: &str,
    lang: Language,
    position: IdentifierPosition,
    context: &str,
) -> Result<(), IdentifierViolation> {
    match identifier_violation(name, lang, position) {
        None => Ok(()),
        Some(reason) => Err(IdentifierViolation {
            name: name.to_string(),
            lang,
            position,
            reason,
            context: context.to_string(),
        }),
    }
}

/// Run the gate at a renderer call site and report a violation without aborting.
///
/// This used to `panic!`, on the reasoning that an illegal identifier is a defect in
/// whatever fed the generator the name rather than a recoverable per-document condition.
/// The reasoning held; the failure mode did not. A panic unwound the whole process from
/// deep inside a docs renderer, so a single bad name in one type's method list took down
/// the stages that run after docs (orphan sweep, formatting, hash finalisation) and
/// produced no ERROR line at all -- 31k lines of clean log and a raw backtrace. Docs are
/// not a release gate, so a name the gate rejects must not cost the run everything that
/// already succeeded; it must cost exactly one loud, greppable ERROR event naming the
/// identifier, the language, the position and the render context. Threading a `Result`
/// out instead would reach ~100 call sites across the signature renderers for no extra
/// signal, since no caller between here and the docs stage can do anything but forward
/// it. Callers that *can* act on the failure use [`check_identifier`].
pub(crate) fn report_identifier_violation(name: &str, lang: Language, position: IdentifierPosition, context: &str) {
    if let Err(violation) = check_identifier(name, lang, position, context) {
        tracing::error!(
            identifier = %violation.name,
            language = ?violation.lang,
            position = violation.position.label(),
            reason = violation.reason,
            render_context = %violation.context,
            "{}",
            violation
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Language;
    use crate::core::ir::{DefaultValue, PrimitiveType, TypeRef};
    use crate::docs::test_helpers::{TEST_CRATE_NAME, TEST_PREFIX, empty_api, make_field};

    // ~keep The identifier gate: rejects a reserved-word collision (Java/Dart's `new`,
    // etc.) that a curated or templated name might introduce. These are the per-language
    // ground truths the tslp `DownloadManager` audit measured against a real compiler
    // (`javac`) and real language specs, not inference -- see reserved_words' doc comment.
    #[test]
    fn test_identifier_violation_rejects_new_in_declaration_position_wherever_it_is_reserved() {
        for lang in [
            Language::Java,
            Language::Dart,
            Language::Php,
            Language::Node,
            Language::Csharp,
        ] {
            assert_eq!(
                identifier_violation("new", lang, IdentifierPosition::Declaration),
                Some("reserved word"),
                "`new` must be rejected in declaration position for {lang:?}"
            );
        }
    }

    /// The tslp crash. `new` in *member* position is legal TypeScript and legal PHP 7+, and
    /// both backends emit it verbatim -- napi's `index.d.ts` really carries
    /// `static new(version: string): DownloadManager`. A position-blind gate called that a
    /// reserved word and panicked the docs run over correct output.
    #[test]
    fn test_identifier_violation_accepts_new_in_member_position_where_the_grammar_allows_it() {
        for lang in [Language::Node, Language::Wasm, Language::Php] {
            assert_eq!(
                identifier_violation("new", lang, IdentifierPosition::Member),
                None,
                "`new` is a legal member name in {lang:?}"
            );
        }
    }

    /// Position-awareness is not a blanket amnesty. Java, C# and Dart all take an
    /// `identifier` in member position, which excludes their reserved words, so the gate
    /// must keep firing there -- these are the positive controls that separate the fix from
    /// "make the gate always pass".
    #[test]
    fn test_identifier_violation_still_rejects_new_in_member_position_where_the_grammar_forbids_it() {
        for lang in [Language::Java, Language::Csharp, Language::Dart] {
            assert_eq!(
                identifier_violation("new", lang, IdentifierPosition::Member),
                Some("reserved word"),
                "`new` is not a legal member name in {lang:?}"
            );
        }
    }

    /// A language whose member slot *is* relaxed still cannot take a keyword as the binding
    /// identifier of a top-level `function` declaration -- which is exactly what
    /// `render_typescript_fn_sig` emits for a free function.
    #[test]
    fn test_identifier_violation_node_relaxation_does_not_reach_declaration_position() {
        assert_eq!(
            identifier_violation("class", Language::Node, IdentifierPosition::Member),
            None
        );
        assert_eq!(
            identifier_violation("class", Language::Node, IdentifierPosition::Declaration),
            Some("reserved word")
        );
    }

    /// Syntactic well-formedness is position-independent: no member slot here accepts an
    /// unquoted name starting with a digit, so relaxing the reserved-word rule must not
    /// relax this one with it.
    #[test]
    fn test_identifier_violation_rejects_malformed_names_in_member_position_too() {
        for lang in [Language::Node, Language::Wasm, Language::Php] {
            assert_eq!(
                identifier_violation("1invalid", lang, IdentifierPosition::Member),
                Some("not a syntactically valid identifier"),
                "{lang:?}"
            );
            assert_eq!(
                identifier_violation("", lang, IdentifierPosition::Member),
                Some("empty identifier"),
                "{lang:?}"
            );
        }
    }

    #[test]
    fn test_identifier_violation_accepts_new_in_languages_where_it_is_not_reserved() {
        // Rust, Go, Kotlin, Zig, and Elixir do not reserve `new` -- this is exactly why
        // `Type::new()`/`func_name(...)`-style constructors are idiomatic there. Elixir's
        // real `DownloadManager.new/1` (the tslp audit's one correct case) depends on this.
        for lang in [
            Language::Rust,
            Language::Go,
            Language::Kotlin,
            Language::Zig,
            Language::Elixir,
        ] {
            for position in [IdentifierPosition::Declaration, IdentifierPosition::Member] {
                assert_eq!(
                    identifier_violation("new", lang, position),
                    None,
                    "`new` must be accepted for {lang:?} in {position:?} position"
                );
            }
        }
    }

    #[test]
    fn test_identifier_violation_csharp_new_only_collides_lowercase() {
        // C#'s real defect (a fabricated static factory instead of a real constructor) is
        // a name-*selection* problem, not a reserved-word one: `func_name` PascalCases to
        // `New`, which is not the reserved `new` and so the gate correctly does not fire on
        // it -- catching this class of bug needs item 1 (modeling the curated
        // constructor), not the gate.
        assert_eq!(
            identifier_violation("new", Language::Csharp, IdentifierPosition::Declaration),
            Some("reserved word")
        );
        assert_eq!(
            identifier_violation("New", Language::Csharp, IdentifierPosition::Declaration),
            None
        );
    }

    #[test]
    fn test_identifier_violation_rejects_python_default_arg_style_keywords() {
        assert_eq!(
            identifier_violation("class", Language::Python, IdentifierPosition::Declaration),
            Some("reserved word")
        );
        assert_eq!(
            identifier_violation("import", Language::Python, IdentifierPosition::Declaration),
            Some("reserved word")
        );
    }

    #[test]
    fn test_identifier_violation_kotlin_default_is_not_reserved() {
        // Regression guard for the existing `default` -> `@JvmStatic fun default()` test
        // fixture: Kotlin does not reserve `default` as a hard keyword.
        assert_eq!(
            identifier_violation("default", Language::Kotlin, IdentifierPosition::Declaration),
            None
        );
    }

    #[test]
    fn test_identifier_violation_rejects_empty_identifier() {
        assert_eq!(
            identifier_violation("", Language::Python, IdentifierPosition::Declaration),
            Some("empty identifier")
        );
    }

    #[test]
    fn test_identifier_violation_rejects_identifier_starting_with_digit() {
        assert_eq!(
            identifier_violation("1invalid", Language::Python, IdentifierPosition::Declaration),
            Some("not a syntactically valid identifier")
        );
    }

    #[test]
    fn test_identifier_violation_accepts_ordinary_identifier_in_every_language() {
        for lang in [
            Language::Python,
            Language::Node,
            Language::Wasm,
            Language::Ruby,
            Language::Php,
            Language::Elixir,
            Language::Go,
            Language::Java,
            Language::Csharp,
            Language::Rust,
            Language::Kotlin,
            Language::KotlinAndroid,
            Language::Swift,
            Language::Dart,
            Language::Gleam,
            Language::Zig,
            Language::R,
            Language::Ffi,
            Language::C,
            Language::Jni,
        ] {
            assert_eq!(
                identifier_violation("classifyLink", lang, IdentifierPosition::Declaration),
                None,
                "an ordinary camelCase identifier must be accepted for {lang:?}"
            );
        }
    }

    /// The gate no longer aborts the process; a rejected name comes back as an inspectable
    /// error whose text carries every field the old panic message did, plus the position
    /// that decided the verdict.
    #[test]
    fn test_check_identifier_returns_a_structured_error_instead_of_panicking() {
        let violation = check_identifier("new", Language::Java, IdentifierPosition::Member, "a method signature")
            .expect_err("`new` is not a legal Java member name");

        assert_eq!(violation.name, "new");
        assert_eq!(violation.lang, Language::Java);
        assert_eq!(violation.position, IdentifierPosition::Member);
        assert_eq!(violation.reason, "reserved word");
        assert_eq!(violation.context, "a method signature");

        let rendered = violation.to_string();
        assert!(rendered.contains("`new`"), "{rendered}");
        assert!(rendered.contains("reserved word"), "{rendered}");
        assert!(rendered.contains("member position"), "{rendered}");
        assert!(rendered.contains("a method signature"), "{rendered}");
        assert!(rendered.contains("Java"), "{rendered}");
    }

    /// The exact tslp input: a napi method named `new`. This must now be `Ok`, and
    /// `report_identifier_violation` must return normally rather than unwinding.
    #[test]
    fn test_check_identifier_accepts_the_napi_static_new_that_used_to_abort_the_docs_run() {
        assert_eq!(
            check_identifier("new", Language::Node, IdentifierPosition::Member, "a method signature"),
            Ok(())
        );
        report_identifier_violation("new", Language::Node, IdentifierPosition::Member, "a method signature");
    }

    /// A violation must not unwind either -- it is reported through `tracing` and rendering
    /// continues, so one bad name costs an ERROR event rather than the whole run.
    #[test]
    fn test_report_identifier_violation_does_not_unwind_on_a_violation() {
        report_identifier_violation("new", Language::Java, IdentifierPosition::Member, "a test context");
        report_identifier_violation(
            "createClient",
            Language::Java,
            IdentifierPosition::Member,
            "a test context",
        );
    }

    #[test]
    fn test_doc_type_with_optional_true_wraps_correctly() {
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::Python, true, TEST_PREFIX),
            "str | None"
        );
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::Node, true, TEST_PREFIX),
            "string | null"
        );
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::Go, true, TEST_PREFIX),
            "*string"
        );
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::Csharp, true, TEST_PREFIX),
            "string?"
        );
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::Ruby, true, TEST_PREFIX),
            "String?"
        );
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::Php, true, TEST_PREFIX),
            "?string"
        );
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::Elixir, true, TEST_PREFIX),
            "String.t() | nil"
        );
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::R, true, TEST_PREFIX),
            "character or NULL"
        );
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::Rust, true, TEST_PREFIX),
            "Option<String>"
        );
        // ~keep A single pointer, not a double pointer: the real cbindgen header declares
        // optional string params as plain `const char *` (nullable in place), e.g. a
        // consumer's generated `<prefix>_create_client` and its `base_url`/`model_hint`
        // params -- confirmed against that generated header, not inferred from the type
        // system. This was the exact `const char**`-vs-header-`const char *` divergence
        // the params-table fix exists to close; pinning the old doubled value here would
        // have re-legalized the bug.
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::Ffi, true, TEST_PREFIX),
            "const char*"
        );
    }

    #[test]
    fn test_doc_type_with_optional_false_is_identity() {
        for lang in [
            Language::Python,
            Language::Node,
            Language::Go,
            Language::Java,
            Language::Rust,
        ] {
            assert_eq!(
                doc_type_with_optional(&TypeRef::String, lang, false, TEST_PREFIX),
                crate::docs::doc_type(&TypeRef::String, lang, TEST_PREFIX),
                "optional=false should be identity for {lang:?}"
            );
        }
    }

    #[test]
    fn test_doc_type_with_optional_does_not_double_wrap_already_optional_type() {
        let already_optional = TypeRef::Optional(Box::new(TypeRef::String));
        assert_eq!(
            doc_type_with_optional(&already_optional, Language::Python, true, TEST_PREFIX),
            "str | None"
        );
        assert_eq!(
            doc_type_with_optional(&already_optional, Language::Rust, true, TEST_PREFIX),
            "Option<String>"
        );
    }

    #[test]
    fn test_doc_type_with_optional_java_boxes_primitive_i32() {
        assert_eq!(
            doc_type_with_optional(
                &TypeRef::Primitive(PrimitiveType::I32),
                Language::Java,
                true,
                TEST_PREFIX
            ),
            "@Nullable Integer"
        );
    }

    #[test]
    fn test_doc_type_with_optional_java_boxes_primitive_bool() {
        assert_eq!(
            doc_type_with_optional(
                &TypeRef::Primitive(PrimitiveType::Bool),
                Language::Java,
                true,
                TEST_PREFIX
            ),
            "@Nullable Boolean"
        );
    }

    #[test]
    fn test_doc_type_with_optional_java_boxes_primitive_f64() {
        assert_eq!(
            doc_type_with_optional(
                &TypeRef::Primitive(PrimitiveType::F64),
                Language::Java,
                true,
                TEST_PREFIX
            ),
            "@Nullable Double"
        );
    }

    #[test]
    fn test_doc_type_with_optional_java_non_primitive_not_double_boxed() {
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::Java, true, TEST_PREFIX),
            "@Nullable String"
        );
    }

    #[test]
    fn test_doc_type_with_optional_named_ffi_is_bare_scalar_handle_not_a_pointer() {
        let ty = TypeRef::Named("ClientConfig".to_string());
        let rendered = doc_type_with_optional(&ty, Language::Ffi, true, TEST_PREFIX);
        assert_eq!(rendered, "HTMAlefHandle");
        assert!(
            !rendered.contains('*'),
            "handle must not be pointer-suffixed: {rendered}"
        );
        assert!(
            !rendered.contains("ClientConfig"),
            "must not name the concrete Rust type: {rendered}"
        );
    }

    #[test]
    fn test_doc_type_with_optional_string_ffi_is_single_pointer_not_double() {
        // Regression: this is the params-table / struct-fields-table half of the
        // `const char**`-vs-header-`const char *` divergence; type_mapping.rs's `Optional`
        // arm covers the other half (a literal `TypeRef::Optional(String)`).
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::Ffi, true, TEST_PREFIX),
            "const char*"
        );
    }

    #[test]
    fn test_doc_type_with_optional_string_jni_currently_emits_c_double_pointer() {
        assert_eq!(
            doc_type_with_optional(&TypeRef::String, Language::Jni, true, TEST_PREFIX),
            "const char**"
        );
    }

    /// ~keep The parameters table (`doc_type_with_optional`) and the function/method
    /// signature line (`type_mapping::doc_type`, via `signatures.rs`) render the same
    /// Named type in two different places on the same generated page. They must agree on
    /// the handle token for a required field exactly as they do for an optional one --
    /// this is the params-table analogue of `ffi_handle_consistency.rs`'s
    /// signature-vs-example cross-check.
    #[test]
    fn test_doc_type_with_optional_agrees_with_doc_type_on_handle_token() {
        let ty = TypeRef::Named("ClientConfig".to_string());
        let required = doc_type(&ty, Language::Ffi, TEST_PREFIX);
        let optional = doc_type_with_optional(&ty, Language::Ffi, true, TEST_PREFIX);
        assert_eq!(
            required, optional,
            "a Named type must render identically whether required or optional"
        );
        assert_eq!(required, "HTMAlefHandle");
    }

    // ~keep The on-error return value depends on the function's return shape; traced from
    // backends/ffi/gen_bindings/{functions/orchestration,helpers}.rs -- see ffi_error_return_phrase's
    // doc comment for the source citations. One case per row of that table.
    #[test]
    fn test_format_error_phrase_ffi_unit_return_reports_negative_one() {
        assert_eq!(
            format_error_phrase("InitError", &TypeRef::Unit, Language::Ffi, TEST_CRATE_NAME),
            "Returns `-1` on error."
        );
    }

    #[test]
    fn test_format_error_phrase_ffi_bytes_return_reports_negative_one() {
        // A Bytes result is always delivered via out-param + i32 status, fallible or not.
        assert_eq!(
            format_error_phrase("ReadError", &TypeRef::Bytes, Language::Ffi, TEST_CRATE_NAME),
            "Returns `-1` on error."
        );
    }

    #[test]
    fn test_format_error_phrase_ffi_named_return_reports_integer_sentinel_not_null() {
        let phrase = format_error_phrase(
            "ParseError",
            &TypeRef::Named("ConversionResult".to_string()),
            Language::C,
            TEST_CRATE_NAME,
        );
        assert_eq!(phrase, "Returns the sentinel handle `0` on error.");
        assert!(
            !phrase.contains("NULL"),
            "a scalar handle has no pointer to compare: {phrase}"
        );
    }

    #[test]
    fn test_format_error_phrase_ffi_string_return_stays_null() {
        // A genuine heap-allocated pointer return really does use NULL on failure.
        assert_eq!(
            format_error_phrase("ReadError", &TypeRef::String, Language::C, TEST_CRATE_NAME),
            "Returns `NULL` on error."
        );
        assert_eq!(
            format_error_phrase(
                "ReadError",
                &TypeRef::Vec(Box::new(TypeRef::String)),
                Language::C,
                TEST_CRATE_NAME
            ),
            "Returns `NULL` on error."
        );
        assert_eq!(
            format_error_phrase(
                "ReadError",
                &TypeRef::Map(Box::new(TypeRef::String), Box::new(TypeRef::String)),
                Language::C,
                TEST_CRATE_NAME
            ),
            "Returns `NULL` on error."
        );
    }

    #[test]
    fn test_format_error_phrase_ffi_numeric_primitive_reports_zero() {
        assert_eq!(
            format_error_phrase(
                "ComputeError",
                &TypeRef::Primitive(PrimitiveType::I32),
                Language::C,
                TEST_CRATE_NAME
            ),
            "Returns `0` on error."
        );
        assert_eq!(
            format_error_phrase(
                "ComputeError",
                &TypeRef::Primitive(PrimitiveType::F64),
                Language::C,
                TEST_CRATE_NAME
            ),
            "Returns `0.0` on error."
        );
        assert_eq!(
            format_error_phrase("ComputeError", &TypeRef::Duration, Language::C, TEST_CRATE_NAME),
            "Returns `0` on error."
        );
    }

    #[test]
    fn test_format_error_phrase_ffi_optional_named_recurses_to_inner_shape() {
        let ty = TypeRef::Optional(Box::new(TypeRef::Named("ClientConfig".to_string())));
        assert_eq!(
            format_error_phrase("AttachError", &ty, Language::C, TEST_CRATE_NAME),
            "Returns the sentinel handle `0` on error."
        );
    }

    #[test]
    fn test_format_error_phrase_jni_currently_emits_fixed_null_phrase() {
        // ~keep Characterisation only: this pins what the Jni arm currently emits, NOT what it
        // ought to emit. Jni is reachable and the phrase is wrong -- a JNI shim throws
        // `<Bridge>Exception` and returns a type-specific zero, never `NULL`. See
        // docs::examples::sample_param_value; expect this test to change when the arm is fixed.
        assert_eq!(
            format_error_phrase("InitError", &TypeRef::Unit, Language::Jni, TEST_CRATE_NAME),
            "Returns `NULL` on error."
        );
        assert_eq!(
            format_error_phrase(
                "ParseError",
                &TypeRef::Named("ConversionResult".to_string()),
                Language::Jni,
                TEST_CRATE_NAME
            ),
            "Returns `NULL` on error."
        );
    }

    #[test]
    fn test_format_default_bool_literal_python_uses_capitalised_form() {
        let api = empty_api();
        let field_true = make_field(
            "flag",
            TypeRef::Primitive(PrimitiveType::Bool),
            false,
            Some(DefaultValue::BoolLiteral(true)),
        );
        let field_false = make_field(
            "flag",
            TypeRef::Primitive(PrimitiveType::Bool),
            false,
            Some(DefaultValue::BoolLiteral(false)),
        );
        assert_eq!(
            format_field_default(&field_true, Language::Python, &api, TEST_PREFIX),
            "`True`"
        );
        assert_eq!(
            format_field_default(&field_false, Language::Python, &api, TEST_PREFIX),
            "`False`"
        );
    }

    #[test]
    fn test_format_default_bool_literal_non_python_uses_lowercase_form() {
        let api = empty_api();
        let field_true = make_field(
            "flag",
            TypeRef::Primitive(PrimitiveType::Bool),
            false,
            Some(DefaultValue::BoolLiteral(true)),
        );
        for lang in [Language::Rust, Language::Java, Language::Go, Language::Node] {
            assert_eq!(
                format_field_default(&field_true, lang, &api, TEST_PREFIX),
                "`true`",
                "bool literal for {lang:?}"
            );
        }
    }

    #[test]
    fn test_format_default_string_literal_all_languages_produce_quoted_form() {
        let api = empty_api();
        let field = make_field(
            "name",
            TypeRef::String,
            false,
            Some(DefaultValue::StringLiteral("hello".to_string())),
        );
        for lang in [
            Language::Python,
            Language::Rust,
            Language::Java,
            Language::Go,
            Language::Node,
        ] {
            assert_eq!(
                format_field_default(&field, lang, &api, TEST_PREFIX),
                "`\"hello\"`",
                "string literal for {lang:?}"
            );
        }
    }

    #[test]
    fn test_format_default_int_literal() {
        let api = empty_api();
        let field = make_field(
            "count",
            TypeRef::Primitive(PrimitiveType::U32),
            false,
            Some(DefaultValue::IntLiteral(42)),
        );
        for lang in [Language::Python, Language::Rust, Language::Java, Language::Node] {
            assert_eq!(
                format_field_default(&field, lang, &api, TEST_PREFIX),
                "`42`",
                "int literal for {lang:?}"
            );
        }
    }

    #[test]
    fn test_format_default_int_literal_on_duration_field_shows_ms_suffix() {
        let api = empty_api();
        let field = make_field(
            "timeout",
            TypeRef::Duration,
            false,
            Some(DefaultValue::IntLiteral(5000)),
        );
        for lang in [Language::Python, Language::Rust, Language::Java, Language::Go] {
            assert_eq!(
                format_field_default(&field, lang, &api, TEST_PREFIX),
                "`5000ms`",
                "duration field should show ms suffix for {lang:?}"
            );
        }
    }

    #[test]
    fn test_format_default_float_literal() {
        let api = empty_api();
        let field = make_field(
            "confidence",
            TypeRef::Primitive(PrimitiveType::F32),
            false,
            Some(DefaultValue::FloatLiteral(0.85)),
        );
        for lang in [Language::Python, Language::Rust, Language::Java] {
            assert_eq!(
                format_field_default(&field, lang, &api, TEST_PREFIX),
                "`0.85`",
                "float literal for {lang:?}"
            );
        }
    }

    #[test]
    fn test_format_default_enum_variant_qualified_python_and_rust() {
        let api = empty_api();
        let field = make_field(
            "style",
            TypeRef::Named("HeadingStyle".to_string()),
            false,
            Some(DefaultValue::EnumVariant("HeadingStyle::Atx".to_string())),
        );
        assert_eq!(
            format_field_default(&field, Language::Python, &api, TEST_PREFIX),
            "`HeadingStyle.ATX`"
        );
        assert_eq!(
            format_field_default(&field, Language::Rust, &api, TEST_PREFIX),
            "`HeadingStyle::Atx`"
        );
        // Java declares the enum constant in SCREAMING_SNAKE, its own convention
        // (`backends/java/gen_bindings/types/enums.rs::gen_enum_class`, which routes the constant
        // through the same `public_casing` authority these docs do). This comment previously said
        // the opposite and was accurate at the time: the generator pushed `variant.name` verbatim,
        // so the docs were correctly describing a generator that was itself wrong. ~keep
        assert_eq!(
            format_field_default(&field, Language::Java, &api, TEST_PREFIX),
            "`HeadingStyle.ATX`"
        );
        assert_eq!(
            format_field_default(&field, Language::Ruby, &api, TEST_PREFIX),
            "`:atx`"
        );
        // PHP's class constant uppercases the whole variant name with no underscores inserted
        // (`backends/php/gen_bindings/types/enums.rs::enum_constant_entries`). ~keep
        assert_eq!(
            format_field_default(&field, Language::Php, &api, TEST_PREFIX),
            "`HeadingStyle::ATX`"
        );
    }

    #[test]
    fn test_format_default_tuple_variant_qualifies_variant_and_renders_arguments() {
        let api = empty_api();
        let field = make_field(
            "kind",
            TypeRef::Named("Kind".to_string()),
            false,
            Some(DefaultValue::TupleVariant(
                "Scaled".to_string(),
                vec![
                    DefaultValue::IntLiteral(5),
                    DefaultValue::StringLiteral("x".to_string()),
                ],
            )),
        );
        assert_eq!(
            format_field_default(&field, Language::Rust, &api, TEST_PREFIX),
            "`Kind::Scaled(5, \"x\")`"
        );
        assert_eq!(
            format_field_default(&field, Language::Python, &api, TEST_PREFIX),
            "`Kind.SCALED(5, \"x\")`"
        );
    }

    #[test]
    fn test_format_default_struct_variant_qualifies_variant_and_renders_named_fields() {
        let api = empty_api();
        let field = make_field(
            "kind",
            TypeRef::Named("Kind".to_string()),
            false,
            Some(DefaultValue::StructVariant(
                "Curated".to_string(),
                vec![
                    ("label".to_string(), DefaultValue::StringLiteral("balanced".to_string())),
                    ("weight".to_string(), DefaultValue::IntLiteral(3)),
                ],
            )),
        );
        assert_eq!(
            format_field_default(&field, Language::Rust, &api, TEST_PREFIX),
            "`Kind::Curated { label: \"balanced\", weight: 3 }`"
        );
        assert_eq!(
            format_field_default(&field, Language::Python, &api, TEST_PREFIX),
            "`Kind.CURATED { label: \"balanced\", weight: 3 }`"
        );
    }

    #[test]
    fn test_format_default_empty_vec_field() {
        let api = empty_api();
        let field = make_field(
            "items",
            TypeRef::Vec(Box::new(TypeRef::String)),
            false,
            Some(DefaultValue::Empty),
        );
        assert_eq!(
            format_field_default(&field, Language::Python, &api, TEST_PREFIX),
            "`[]`"
        );
        assert_eq!(
            format_field_default(&field, Language::Rust, &api, TEST_PREFIX),
            "`vec![]`"
        );
        assert_eq!(
            format_field_default(&field, Language::Java, &api, TEST_PREFIX),
            "`Collections.emptyList()`"
        );
        assert_eq!(format_field_default(&field, Language::Go, &api, TEST_PREFIX), "`nil`");
        assert_eq!(
            format_field_default(&field, Language::Csharp, &api, TEST_PREFIX),
            "`new List<string>()`"
        );
        assert_eq!(format_field_default(&field, Language::R, &api, TEST_PREFIX), "`list()`");
        assert_eq!(format_field_default(&field, Language::Ruby, &api, TEST_PREFIX), "`[]`");
        assert_eq!(
            format_field_default(&field, Language::Elixir, &api, TEST_PREFIX),
            "`[]`"
        );
        assert_eq!(format_field_default(&field, Language::Ffi, &api, TEST_PREFIX), "`NULL`");
    }

    #[test]
    fn test_format_default_empty_map_field() {
        let api = empty_api();
        let field = make_field(
            "attributes",
            TypeRef::Map(Box::new(TypeRef::String), Box::new(TypeRef::String)),
            false,
            Some(DefaultValue::Empty),
        );
        assert_eq!(
            format_field_default(&field, Language::Python, &api, TEST_PREFIX),
            "`{}`"
        );
        assert_eq!(
            format_field_default(&field, Language::Rust, &api, TEST_PREFIX),
            "`HashMap::new()`"
        );
        assert_eq!(
            format_field_default(&field, Language::Java, &api, TEST_PREFIX),
            "`Collections.emptyMap()`"
        );
        assert_eq!(format_field_default(&field, Language::Go, &api, TEST_PREFIX), "`nil`");
        assert_eq!(
            format_field_default(&field, Language::Elixir, &api, TEST_PREFIX),
            "`%{}`"
        );
        assert_eq!(
            format_field_default(&field, Language::Csharp, &api, TEST_PREFIX),
            "`new Dictionary<string, string>()`"
        );
    }

    #[test]
    fn test_format_default_none_on_optional_field() {
        let api = empty_api();
        let field = make_field("label", TypeRef::String, true, Some(DefaultValue::None));
        assert_eq!(
            format_field_default(&field, Language::Python, &api, TEST_PREFIX),
            "`None`"
        );
        assert_eq!(
            format_field_default(&field, Language::Node, &api, TEST_PREFIX),
            "`null`"
        );
        assert_eq!(format_field_default(&field, Language::Go, &api, TEST_PREFIX), "`nil`");
        assert_eq!(
            format_field_default(&field, Language::Rust, &api, TEST_PREFIX),
            "`None`"
        );
        assert_eq!(format_field_default(&field, Language::Ffi, &api, TEST_PREFIX), "`NULL`");
        assert_eq!(format_field_default(&field, Language::R, &api, TEST_PREFIX), "`NULL`");
    }

    #[test]
    fn test_format_default_none_on_non_optional_field_returns_dash() {
        let api = empty_api();
        let field = make_field(
            "count",
            TypeRef::Primitive(PrimitiveType::U32),
            false,
            Some(DefaultValue::None),
        );
        assert_eq!(format_field_default(&field, Language::Python, &api, TEST_PREFIX), "—");
    }

    #[test]
    fn test_format_default_empty_duration_shows_zero_ms_for_non_rust() {
        let api = empty_api();
        let field = make_field("timeout", TypeRef::Duration, false, Some(DefaultValue::Empty));
        assert_eq!(
            format_field_default(&field, Language::Python, &api, TEST_PREFIX),
            "`0ms`"
        );
        assert_eq!(format_field_default(&field, Language::Java, &api, TEST_PREFIX), "`0ms`");
        assert_eq!(format_field_default(&field, Language::Go, &api, TEST_PREFIX), "`0ms`");
    }

    #[test]
    fn test_format_default_empty_duration_rust_shows_duration_default() {
        let api = empty_api();
        let field = make_field("timeout", TypeRef::Duration, false, Some(DefaultValue::Empty));
        assert_eq!(
            format_field_default(&field, Language::Rust, &api, TEST_PREFIX),
            "`Duration::default()`"
        );
    }

    #[test]
    fn test_format_default_multiline_raw_string_collapsed() {
        let api = empty_api();
        let mut field = make_field("config", TypeRef::String, false, None);
        field.default = Some("line1\nline2\nline3".to_string());
        let result = format_field_default(&field, Language::Python, &api, TEST_PREFIX);
        assert_eq!(result, "`line1 line2 line3`", "multiline defaults must be collapsed");
        assert!(!result.contains('\n'), "backtick code span must be single-line");
    }

    #[test]
    fn test_format_default_raw_string_with_extra_spaces_collapsed() {
        let api = empty_api();
        let mut field = make_field("config", TypeRef::String, false, None);
        field.default = Some("value   with    spaces".to_string());
        let result = format_field_default(&field, Language::Python, &api, TEST_PREFIX);
        assert!(result.contains("value"), "default should be wrapped");
        assert!(!result.contains("   "), "extra spaces should be normalized");
    }

    #[test]
    fn test_format_default_string_literal_with_newlines_collapsed() {
        let api = empty_api();
        let field = make_field(
            "marker_format",
            TypeRef::String,
            false,
            Some(DefaultValue::StringLiteral("line1\nline2\nline3".to_string())),
        );
        let result = format_field_default(&field, Language::Python, &api, TEST_PREFIX);
        assert_eq!(result, "`\"line1 line2 line3\"`");
        assert!(!result.contains('\n'), "newlines must be removed");
    }

    #[test]
    fn test_format_default_string_literal_with_pipes_and_newlines() {
        let api = empty_api();
        let field = make_field(
            "marker_format",
            TypeRef::String,
            false,
            Some(DefaultValue::StringLiteral(
                "\n\n<!-- PAGE {page_num} -->\n\n".to_string(),
            )),
        );
        let result = format_field_default(&field, Language::Python, &api, TEST_PREFIX);
        assert!(!result.contains('\n'), "newlines must be collapsed");
        let without_backticks = result.trim_matches('`').trim_matches('"');
        assert!(
            without_backticks.starts_with("<!-- PAGE"),
            "normalized: {}",
            without_backticks
        );
    }

    #[test]
    fn test_format_default_string_literal_with_extra_spaces() {
        let api = empty_api();
        let field = make_field(
            "options",
            TypeRef::String,
            false,
            Some(DefaultValue::StringLiteral("option1  option2   option3".to_string())),
        );
        let result = format_field_default(&field, Language::Python, &api, TEST_PREFIX);
        assert!(!result.contains("  "), "multiple spaces must be normalized: {}", result);
    }
}
