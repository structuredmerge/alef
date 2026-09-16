//! NAPI trait-bridge native-array callback args + type-conditional return decoding (#1304).
//!
//! Neutral fixture: a plugin trait `Embedder` with three async methods, all taking a
//! `Vec<String>` param:
//!   - `embed`  returns `Vec<Vec<f32>>` — arg is native-encodable; `f32` has no
//!     `FromNapiValue` impl in napi-rs, so the return decodes via the f64-analog bridge
//!     (`Vec<Vec<f64>>` via `FromNapiValue`, then an element-wise `as f32` cast) instead of
//!     a JSON round-trip.
//!   - `tag`    returns `Vec<String>` — both arg and return are napi-native end to end, no
//!     bridging needed.
//!   - `describe` returns the known serde struct `Doc` — decodes via `Doc`'s own `JsDoc`
//!     native DTO (`NapiBridgeGenerator::plan_return_decode`'s `Named` case), not a JSON
//!     round-trip; see `named_struct_return_type_decodes_via_native_js_dto_not_json_fallback`
//!     for why this is NOT the same claim the test's original name made.
//!
//! Asserts (post-xberg#1636: argument marshalling and return decoding now happen inside each
//! method's `ThreadsafeFunction`, built once in `new()` — see `tsfn_init_source`/the TSFN type
//! alias assertions below — not inline in the trait-impl method body):
//!   (a) every method's `texts: Vec<String>` argument is passed as a native JS array via
//!       `ToNapiValue`, never `format!("{:?}", texts)`.
//!   (b) `tag`'s return decodes via the TSFN's `AlefJsReply<Vec<String>>` Return type, no
//!       JSON round-trip.
//!   (c) `embed`'s return decodes via the f64 analog + element-wise `as f32` cast, matching
//!       its `Vec<Vec<_>>` nesting depth, and does NOT fall back to a JSON round-trip.
//!   (d) `describe`'s return (`Doc` struct) decodes via its native `JsDoc` DTO.

use alef::core::config::{BridgeBinding, TraitBridgeConfig};
use alef::core::ir::*;

fn make_field(name: &str, ty: TypeRef) -> FieldDef {
    FieldDef {
        name: name.to_string(),
        ty,
        ..Default::default()
    }
}

fn make_param(name: &str, ty: TypeRef, is_ref: bool) -> ParamDef {
    ParamDef {
        name: name.to_string(),
        ty,
        is_ref,
        ..Default::default()
    }
}

fn async_method(name: &str, return_type: TypeRef) -> MethodDef {
    MethodDef {
        name: name.to_string(),
        params: vec![make_param("texts", TypeRef::Vec(Box::new(TypeRef::String)), false)],
        return_type,
        receiver: Some(ReceiverKind::Ref),
        cfg: None,
        error_type: Some("Error".to_string()),
        is_async: true,
        ..Default::default()
    }
}

/// `embed(texts) -> Vec<Vec<f32>>`, `tag(texts) -> Vec<String>`,
/// `describe(texts) -> Doc` (known serde struct).
fn embedder_trait() -> TypeDef {
    TypeDef {
        name: "Embedder".to_string(),
        rust_path: "test_lib::Embedder".to_string(),
        is_trait: true,
        is_opaque: true,
        methods: vec![
            async_method(
                "embed",
                TypeRef::Vec(Box::new(TypeRef::Vec(Box::new(TypeRef::Primitive(PrimitiveType::F32))))),
            ),
            async_method("tag", TypeRef::Vec(Box::new(TypeRef::String))),
            async_method("describe", TypeRef::Named("Doc".to_string())),
        ],
        ..Default::default()
    }
}

fn embedder_api() -> ApiSurface {
    let mut doc = TypeDef {
        name: "Doc".to_string(),
        rust_path: "test_lib::Doc".to_string(),
        has_serde: true,
        fields: vec![make_field("text", TypeRef::String)],
        ..Default::default()
    };
    doc.is_return_type = true;

    ApiSurface {
        crate_name: "test_lib".to_string(),
        version: "0.1.0".to_string(),
        types: vec![embedder_trait(), doc],
        ..Default::default()
    }
}

fn embedder_bridge_cfg() -> TraitBridgeConfig {
    TraitBridgeConfig {
        trait_name: "Embedder".to_string(),
        register_fn: Some("register_embedder".to_string()),
        registry_getter: Some("test_lib::registry::get".to_string()),
        super_trait: Some("Plugin".to_string()),
        bind_via: BridgeBinding::FunctionParam,
        ..Default::default()
    }
}

/// Extracts the source for a single `async fn <name>(...)` method body from the full
/// bridge `impl` code, up to (but not including) the next `async fn` (or end of string).
fn method_source<'a>(code: &'a str, name: &str) -> &'a str {
    let marker = format!("async fn {name}(");
    let start = code
        .find(&marker)
        .unwrap_or_else(|| panic!("method `{name}` not found in:\n{code}"));
    let rest = &code[start..];
    match rest[marker.len()..].find("async fn ") {
        Some(next) => &rest[..marker.len() + next],
        None => rest,
    }
}

fn gen_bridge_code() -> String {
    let trait_def = embedder_trait();
    let api = embedder_api();
    alef::backends::napi::trait_bridge::gen_trait_bridge(
        &trait_def,
        &embedder_bridge_cfg(),
        "test_lib",
        "TestLibError",
        "TestLibError::from({msg})",
        &api,
    )
    .expect("gen_trait_bridge must succeed for Embedder")
    .code
}

/// Extracts the source for one method's `ThreadsafeFunction` field initializer (built in
/// `new()`) -- where argument marshalling now lives (moved, with #1636's rewrite, out of the
/// trait-impl method body and into the `build_callback` closure `new()` builds eagerly on the
/// JS thread). Scoped to `"<name>_tsfn: "` up to the next such marker.
fn tsfn_init_source<'a>(code: &'a str, name: &str) -> &'a str {
    let marker = format!("{name}_tsfn: ");
    // `rfind`, not `find`: the SAME marker text also appears earlier as the struct's plain
    // field DECLARATION (`embed_tsfn: JsEmbedderBridgeEmbedTsfn,`) before the constructor's
    // actual initializer block (`embed_tsfn: { let __f = ...`) -- the initializer is always the
    // later occurrence.
    let start = code
        .rfind(&marker)
        .unwrap_or_else(|| panic!("`{name}_tsfn` field initializer not found in:\n{code}"));
    let rest = &code[start..];
    match rest[marker.len()..].find("_tsfn: ") {
        Some(next) => &rest[..marker.len() + next],
        None => rest,
    }
}

#[test]
fn vec_string_param_is_passed_as_native_js_array_for_every_method() {
    let code = gen_bridge_code();
    for name in ["embed", "tag", "describe"] {
        let m = tsfn_init_source(&code, name);
        assert!(
            m.contains("napi::bindgen_prelude::ToNapiValue::to_napi_value(ctx.env.raw(), texts.clone())"),
            "`{name}`'s `texts: Vec<String>` arg must be passed as a native JS array via the \
             threadsafe-function closure:\n{m}"
        );
        assert!(
            !m.contains("format!(\"{:?}\", texts)"),
            "`{name}`'s `texts` arg must NOT be Debug-string encoded:\n{m}"
        );
    }
}

#[test]
fn native_decodable_return_type_decodes_via_from_napi_value() {
    let code = gen_bridge_code();
    // The native decode now happens inside `AlefJsReply<T>::from_napi_value`/`settle()` (see
    // `support::trait_bridge_runtime_def`), keyed purely by the TSFN's `Return` type parameter
    // -- there is no per-method `FromNapiValue::from_napi_value(..)` call left in the generated
    // code to assert on. What the generator DOES choose per return type is which `T` the TSFN
    // decodes into and how the trait-impl method converts it, so assert on those instead.
    assert!(
        code.contains("type JsEmbedderBridgeTagTsfn = napi::threadsafe_function::ThreadsafeFunction<\n    (Vec<String>,),\n    AlefJsReply<Vec<String>>,"),
        "`tag`'s `Vec<String>` return is fully napi-native: the TSFN must decode straight into \
         `AlefJsReply<Vec<String>>`, not a JSON intermediate:\n{code}"
    );
    let tag = method_source(&code, "tag");
    assert!(
        tag.contains("let __decoded: Vec<String> ="),
        "`tag`'s trait-impl method must bind the settled reply at the native `Vec<String>` \
         type with no further conversion:\n{tag}"
    );
    assert!(
        !tag.contains("coerce_to_string()") && !tag.contains("serde_json::from_str"),
        "`tag`'s native return must NOT go through the JSON string fallback:\n{tag}"
    );
}

#[test]
fn f32_leaved_return_type_decodes_via_f64_bridge_and_elementwise_cast() {
    let code = gen_bridge_code();
    assert!(
        code.contains("type JsEmbedderBridgeEmbedTsfn = napi::threadsafe_function::ThreadsafeFunction<\n    (Vec<String>,),\n    AlefJsReply<Vec<Vec<f64>>>,"),
        "`embed`'s `Vec<Vec<f32>>` return must decode natively via the f64 analog TSFN Return \
         type (FromNapiValue has no impl for f32, but does for f64):\n{code}"
    );
    let embed = method_source(&code, "embed");
    assert!(
        embed.contains(": Vec<Vec<f64>>"),
        "`embed`'s decode must be typed as the f64 analog `Vec<Vec<f64>>`, matching the \
         `Vec<Vec<f32>>` return's nesting depth:\n{embed}"
    );
    assert!(
        embed.contains("__decoded.into_iter().map(|v| v.into_iter().map(|v| v as f32).collect()).collect()"),
        "`embed` must cast the decoded f64 values back to f32 element-wise at the correct \
         Vec<Vec<_>> nesting depth:\n{embed}"
    );
    assert!(
        !embed.contains("coerce_to_string()") && !embed.contains("serde_json::from_str"),
        "`embed`'s return must NOT go through the JSON string fallback now that the f64 \
         bridge makes it natively decodable:\n{embed}"
    );
}

/// Regression guard for the ORIGINAL claim this test made under the old body-generation
/// design (return-type branching must not touch struct returns, which had no `Js{T}` native
/// decode path and always JSON round-tripped). Under the #1636 rewrite, `Doc` — a known,
/// serde-enabled, non-opaque struct — IS one of `NapiBridgeGenerator::plan_return_decode`'s
/// `Named` cases: the TSFN decodes straight into the binding's own `JsDoc` DTO (which has a
/// real `FromNapiValue` via `#[napi(object)]`) and converts through `From<JsDoc> for
/// test_lib::Doc`, no JSON round-trip at all. That's strictly better fidelity than the old
/// `coerce_to_string`/`serde_json::from_str` path this test used to assert on; the test name
/// (and this doc) stay as a marker of what changed and why, rather than silently flipping the
/// assertion with no explanation.
#[test]
fn named_struct_return_type_decodes_via_native_js_dto_not_json_fallback() {
    let code = gen_bridge_code();
    assert!(
        code.contains("type JsEmbedderBridgeDescribeTsfn = napi::threadsafe_function::ThreadsafeFunction<\n    (Vec<String>,),\n    AlefJsReply<JsDoc>,"),
        "`describe`'s `Doc` struct return is a known native-marshalled struct: the TSFN's \
         Return must decode into `AlefJsReply<JsDoc>`, not a JSON intermediate:\n{code}"
    );
    // Bounded to the `describe` method's own body text (not `method_source`, which -- since
    // `describe` is the LAST async method in this fixture -- runs to the end of the whole file
    // when there's no following `async fn` boundary, sweeping in unrelated later methods like
    // the Plugin super-trait's `version_on_js_thread` and its own legitimate
    // `coerce_to_string()`/JSON fallback).
    let describe_tail = &code[code.find("async fn describe(").expect("describe method present")..];
    let describe = &describe_tail[..describe_tail
        .find("\n    }\n")
        .map(|i| i + 6)
        .unwrap_or(describe_tail.len())];
    assert!(
        describe.contains("let __result = test_lib::Doc::from(__decoded);"),
        "`describe` must convert the decoded `JsDoc` into `Doc` via `From`, not \
         `serde_json::from_value`:\n{describe}"
    );
    assert!(
        !describe.contains("serde_json::from_value") && !describe.contains("coerce_to_string()"),
        "`describe` must NOT go through a JSON round-trip now that `Doc` decodes natively via \
         its `JsDoc` DTO:\n{describe}"
    );
}
