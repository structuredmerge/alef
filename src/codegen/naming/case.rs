//! Language-agnostic mechanical case conversion.
//!
//! Nothing here knows about a [`crate::core::config::Language`] or a name surface; these are the
//! primitives the surface modules compose. Keeping them separate is what lets a surface change
//! its policy without touching the transforms, and vice versa. ~keep

use heck::{ToPascalCase, ToShoutySnakeCase};

/// Apply initialism uppercasing to a PascalCase name using the provided list.
///
/// Scans word boundaries in the PascalCase string and replaces any run of
/// characters that matches a known initialism (case-insensitively) with the
/// canonical form from the list. For example `ImageUrl` becomes `ImageURL`,
/// `UserId` becomes `UserID`, and `GraphQlRouteConfig` becomes `GraphQLRouteConfig`.
pub(super) fn apply_initialisms(name: &str, list: &[&str]) -> String {
    if name.is_empty() {
        return name.to_string();
    }

    let mut words: Vec<&str> = Vec::new();
    let mut word_start = 0;
    let bytes = name.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i].is_ascii_uppercase() {
            words.push(&name[word_start..i]);
            word_start = i;
        }
    }
    words.push(&name[word_start..]);

    let mut result = String::with_capacity(name.len());
    let mut i = 0;
    while i < words.len() {
        let mut matched = false;
        for span in (1..=(words.len() - i)).rev() {
            let candidate: String = words[i..i + span].concat();
            let candidate_upper = candidate.to_ascii_uppercase();
            if let Some(&canonical) = list.iter().find(|&&s| s.to_ascii_uppercase() == candidate_upper) {
                result.push_str(canonical);
                i += span;
                matched = true;
                break;
            }
        }
        if !matched {
            result.push_str(words[i]);
            i += 1;
        }
    }
    result
}

/// Normalize 3+ letter acronyms at the start of a name to PascalCase.
///
/// C# convention: 3+ letter acronyms use PascalCase (Uri, Xml, Json) not all-caps (URI, XML, JSON).
/// This function detects names like "URI", "XML", "JSON" and converts them to "Uri", "Xml", "Json".
/// Leaves already-correct names like "Uri" unchanged, and preserves non-acronym names.
///
/// Examples:
/// - `URI`  → `Uri`  (acronym → PascalCase)
/// - `Uri`  → `Uri`  (already correct)
/// - `XML`  → `Xml`
/// - `Xml`  → `Xml`
/// - `JSON` → `Json`
/// - `Json` → `Json`
/// - `HttpStatus` → `HttpStatus` (not an acronym)
pub(super) fn normalize_acronym_to_pascalcase(name: &str) -> String {
    if name.is_empty() {
        return name.to_string();
    }

    if name.len() >= 3 && name.chars().all(|c| c.is_ascii_uppercase()) {
        let mut result = String::with_capacity(name.len());
        result.push(name.chars().next().unwrap().to_ascii_uppercase());
        result.extend(name.chars().skip(1).map(|c| c.to_ascii_lowercase()));
        return result;
    }

    name.to_string()
}

/// Convert a Rust type name to class name convention for target language.
pub fn to_class_name(name: &str) -> String {
    name.to_pascal_case()
}

/// Convert to SCREAMING_SNAKE for constants.
pub fn to_constant_name(name: &str) -> String {
    name.to_shouty_snake_case()
}

/// A trailing lowercase run shorter than this, immediately after a 2+ letter uppercase run,
/// is treated as a stylization suffix of that acronym rather than the start of a new word.
///
/// This is what tells `RDFa` (acronym `RDF` + a single stylized `a`, one word: `rdfa`) apart
/// from `IOError` (acronym `IO` + the real word `Error`, two words: `io_error`) — both have an
/// uppercase run immediately followed by lowercase letters, and the only structural difference
/// is how many. One trailing letter is never an English word on its own; two or more plausibly
/// is, so `2` is the threshold rather than `1` or `3`. ~keep
const MIN_TRAILING_WORD_LEN: usize = 2;

/// Convert a PascalCase or mixed-case name to snake_case with correct acronym handling.
///
/// Use this instead of `heck::ToSnakeCase` when the input is a PascalCase Rust type or
/// enum variant name — `heck` inserts an underscore before every uppercase letter, which
/// incorrectly splits acronym-style names like `HTMLParser` into `h_t_m_l_parser`.
///
/// Rules:
/// - A run of consecutive uppercase letters is treated as a single acronym word.
/// - If the run is followed by at least [`MIN_TRAILING_WORD_LEN`] lowercase letters, the last
///   uppercase char begins the next word (e.g. `XMLHttp` → `xml_http`).
/// - If the run is followed by fewer than [`MIN_TRAILING_WORD_LEN`] lowercase letters, the
///   whole run plus that short suffix is kept as one word (e.g. `RDFa` → `rdfa`) — see
///   [`MIN_TRAILING_WORD_LEN`] for why.
/// - A single uppercase letter followed by lowercase is always a normal word start, regardless
///   of the rule above (e.g. `MyType` → `my_type`, not one word).
///
/// Examples:
/// - `MyType`         → `my_type`
/// - `Rdfa`           → `rdfa`
/// - `RDFa`           → `rdfa`
/// - `HTMLParser`     → `html_parser`
/// - `XMLHttpRequest` → `xml_http_request`
/// - `IOError`        → `io_error`
/// - `URLPath`        → `url_path`
/// - `JSONLD`         → `jsonld`
pub fn pascal_to_snake(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = name.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(n + 4);
    let mut i = 0;
    while i < n {
        let ch = chars[i];
        if ch.is_ascii_uppercase() {
            let run_start = i;
            while i < n && chars[i].is_ascii_uppercase() {
                i += 1;
            }
            let run_end = i;
            let run_len = run_end - run_start;
            if run_len == 1 {
                if !out.is_empty() {
                    out.push('_');
                }
                out.extend(chars[run_start].to_lowercase());
            } else {
                let trailing_lowercase_len = chars[i..].iter().take_while(|c| c.is_ascii_lowercase()).count();
                let split = if trailing_lowercase_len >= MIN_TRAILING_WORD_LEN {
                    run_len - 1
                } else {
                    run_len
                };
                if !out.is_empty() {
                    out.push('_');
                }
                for &c in chars.iter().skip(run_start).take(split) {
                    out.extend(c.to_lowercase());
                }
                if split < run_len {
                    out.push('_');
                    out.extend(chars[run_start + split].to_lowercase());
                }
            }
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

/// Convert a PascalCase name to SCREAMING_SNAKE_CASE with correct acronym handling.
///
/// Examples:
/// - `MyType`     → `MY_TYPE`
/// - `Rdfa`       → `RDFA`
/// - `HTMLParser` → `HTML_PARSER`
pub fn pascal_to_screaming_snake(name: &str) -> String {
    pascal_to_snake(name).to_ascii_uppercase()
}

/// Capitalize a single already-lowercase segment produced by [`pascal_to_snake`]'s
/// underscore-split, leaving every character after the first untouched.
fn capitalize_segment(segment: &str) -> String {
    let mut chars = segment.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Convert a PascalCase or mixed-case name to PascalCase with correct acronym handling.
///
/// Segments the name exactly as [`pascal_to_snake`] does (so an acronym plus a short
/// stylization suffix stays one word, per [`MIN_TRAILING_WORD_LEN`]), then capitalizes each
/// segment. Use this instead of `heck::ToPascalCase` wherever the input is a PascalCase Rust
/// type or enum variant name.
///
/// Examples:
/// - `MyType`     → `MyType`
/// - `RDFa`       → `Rdfa`
/// - `IOError`    → `IoError`
/// - `HTMLParser` → `HtmlParser`
pub fn pascal_to_pascal(name: &str) -> String {
    pascal_to_snake(name).split('_').map(capitalize_segment).collect()
}

/// Convert a PascalCase or mixed-case name to camelCase with correct acronym handling.
///
/// Same segmentation as [`pascal_to_pascal`], but the first segment is left lowercase instead
/// of capitalized. Use this instead of `heck::ToLowerCamelCase` wherever the input is a
/// PascalCase Rust type or enum variant name.
///
/// Examples:
/// - `MyType`     → `myType`
/// - `RDFa`       → `rdfa`
/// - `IOError`    → `ioError`
/// - `HTMLParser` → `htmlParser`
pub fn pascal_to_camel(name: &str) -> String {
    let snake = pascal_to_snake(name);
    let mut segments = snake.split('_');
    let mut out = String::with_capacity(snake.len());
    if let Some(first) = segments.next() {
        out.push_str(first);
    }
    for segment in segments {
        out.push_str(&capitalize_segment(segment));
    }
    out
}

/// Join a name's `_`-separated segments by capitalizing the first character after each
/// underscore, leaving every other character's case untouched.
///
/// This is deliberately NOT [`super::languages::to_node_name`] (heck's `ToLowerCamelCase`), and
/// the difference is load-bearing. heck re-splits an existing case run and drops a leading
/// underscore, so it rewrites `my_URL` to `myUrl` and `_raw` to `raw`; this transform yields
/// `myURL` and `Raw`. A caller whose output must match a name some *other* generator already
/// emitted — a fixture JSON key echoing a serde wire name, an identifier flutter_rust_bridge
/// wrote into its own Dart output — needs the preserving form, because heck's extra splitting
/// silently produces a name that does not exist on the other side and the mismatch surfaces only
/// as a failing generated test.
///
/// A caller naming an identifier *alef itself* emits must not use this. Go through the surface
/// helper for that language ([`super::host::public_host_identifier`] or `to_node_name`) so the
/// generator and the emitter cannot disagree. ~keep
pub fn underscore_camel_case(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    let mut capitalize_next = false;
    for ch in name.chars() {
        if ch == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }
    result
}
