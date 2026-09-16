use super::visitor_bridge::{
    UnsupportedArg, build_napi_args, f32_bridge_cast_expr, f32_bridge_target, is_napi_decodable,
};
use crate::codegen::generators::trait_bridge::{
    TraitBridgeGenerator, TraitBridgeSpec, format_type_ref, host_function_path, is_native_marshalled_struct,
    to_camel_case,
};
use crate::core::ir::{ApiSurface, MethodDef, ParamDef, TypeRef};
use heck::ToUpperCamelCase;
use std::cell::RefCell;
use std::collections::HashMap;

pub struct NapiBridgeGenerator {
    /// Core crate import path (e.g., `"sample_core"`).
    pub core_import: String,
    /// Map of type name → fully-qualified Rust path for type references.
    pub type_paths: HashMap<String, String>,
    /// Error type name (e.g., `"SampleCrateError"`).
    pub error_type: String,
    /// Callback-param type names that get NATIVE-object marshalling — known serde structs per
    /// the shared [`crate::codegen::generators::trait_bridge::is_native_marshalled_struct`] rule.
    /// For such a param the bridge constructs the binding's native JS object (the
    /// `#[napi(object)]` DTO wrapper, via the same `From<core::T>` conversion used for return
    /// values) and hands THAT to the host method, instead of serializing the param to a string.
    /// Enums, opaque/handle types, and excluded/unknown `Named` params are absent and keep their
    /// prior representation.
    pub struct_param_types: std::collections::HashSet<String>,
    /// Rust-defaulted trait methods the bridge forwards to the host when the JS
    /// object defines them. Presence is now `self.<m>_tsfn.is_some()` (the per-method
    /// `ThreadsafeFunction` field is `Option<..>` for these, a plain field for required
    /// methods) instead of a separately cached `has_<method>: bool` — see
    /// [`Self::extra_bridge_fields`].
    pub forwardable_defaulted: std::collections::HashSet<String>,
    /// Node type-name prefix (e.g. `"Js"`) used to name the native DTO wrapper for a struct param
    /// (`Js{TypeName}`), matching the binding's emitted `#[napi(object)]` struct.
    pub type_prefix: String,
    /// The whole API surface, for [`is_native_marshalled_struct`] lookups on RETURN types —
    /// deciding whether a `Named`/`Vec<Named>`/`Optional<Named>` return decodes through the
    /// binding's native `Js{T}` DTO (`FromNapiValue` + `From<Js{T}> for core::T`) or falls back
    /// to a `serde_json::Value` decode. Param-side marshalling uses `struct_param_types`
    /// (computed once, shared with every backend); this is the return-side counterpart, backend
    /// local because only napi needs the distinction (every other backend's return path already
    /// goes through the FFI/JSON boundary uniformly).
    pub api: ApiSurface,
    /// Sites where [`build_napi_args`] could not marshal a parameter natively — see
    /// [`UnsupportedArg`]. Checked by `gen_trait_bridge` after all method bodies are generated;
    /// a non-empty list is a hard `anyhow::bail!`, not a silent runtime fallback.
    pub(crate) unsupported_args: RefCell<Vec<UnsupportedArg>>,
    /// Private inherent-method bodies emitted alongside the trait impl: today only the
    /// `<method>_on_js_thread` fast-path bodies for synchronous methods (see
    /// [`Self::gen_sync_method_body`]). Collected here because
    /// [`TraitBridgeGenerator::gen_sync_method_body`] can only return the trait-impl method's
    /// own body string; `gen_trait_bridge` drains this into a second `impl { .. }` block after
    /// the trait impl.
    pub(crate) extra_impl_items: RefCell<Vec<String>>,
}

impl NapiBridgeGenerator {
    /// Every own method (required or Rust-defaulted/forwardable) that gets a per-method
    /// `ThreadsafeFunction` field — i.e. every bridged trait method, sync or async alike. Sync
    /// methods use their TSFN only on the off-JS-thread slow path (see
    /// [`Self::gen_sync_method_body`]); async methods use it for every call.
    fn own_methods<'a>(&self, spec: &'a TraitBridgeSpec) -> Vec<&'a MethodDef> {
        spec.trait_def
            .methods
            .iter()
            .filter(|m| {
                m.trait_source.is_none() && (!m.has_default_impl || self.forwardable_defaulted.contains(&m.name))
            })
            .collect()
    }

    /// Every method that gets a per-method `ThreadsafeFunction` field: [`Self::own_methods`]
    /// PLUS the three synthetic `Plugin` lifecycle methods (`version`/`initialize`/`shutdown`)
    /// when a super-trait is configured. The lifecycle methods aren't part of
    /// `spec.trait_def.methods` — `gen_bridge_plugin_impl` (shared cross-backend code)
    /// synthesizes them and calls `gen_sync_method_body` on them directly, bypassing
    /// `own_methods`/the trait-impl generator entirely, so they need the exact same
    /// `MethodDef`s [`gen_bridge_plugin_impl`] uses (via `plugin_lifecycle_methods`) or their
    /// struct-field/alias names would silently disagree with what `gen_sync_method_body`
    /// references at `self.<name>_tsfn`.
    fn tsfn_methods(&self, spec: &TraitBridgeSpec) -> Vec<MethodDef> {
        let mut methods: Vec<MethodDef> = self.own_methods(spec).into_iter().cloned().collect();
        if spec.bridge_config.super_trait.is_some() {
            let version_is_fallible = TraitBridgeGenerator::plugin_version_is_fallible(self);
            methods.extend(crate::codegen::generators::trait_bridge::plugin_lifecycle_methods(
                spec,
                version_is_fallible,
            ));
        }
        methods
    }

    fn is_forwardable(&self, method: &MethodDef) -> bool {
        self.forwardable_defaulted.contains(&method.name)
    }

    /// `core::T` is a "native marshalled struct" per the shared predicate — the binding emits a
    /// `Js{T}` DTO (`#[napi(object)]`, `FromNapiValue`) and a `From<Js{T}> for core::T` /
    /// `From<core::T> for Js{T}` pair, so a JS return value in that shape can decode through it
    /// instead of a JSON round-trip.
    fn is_native_return_type(&self, name: &str) -> bool {
        is_native_marshalled_struct(name, &self.api)
    }

    /// Named enum types safe to JSON-encode as a callback argument — see
    /// [`super::visitor_bridge::json_safe_named_types`].
    fn json_safe_named_types(&self) -> std::collections::HashSet<String> {
        super::visitor_bridge::json_safe_named_types(&self.api)
    }

    fn js_dto_type(&self, core_name: &str) -> String {
        format!("{}{}", self.type_prefix, core_name)
    }

    fn tsfn_field_name(method: &MethodDef) -> String {
        format!("{}_tsfn", method.name)
    }

    fn tsfn_alias_name(&self, spec: &TraitBridgeSpec, method: &MethodDef) -> String {
        format!("{}{}Tsfn", spec.wrapper_name(), method.name.to_upper_camel_case())
    }

    /// Tuple type of the OWNED payload sent into a method's threadsafe function, in
    /// declaration-param order. `()` for a zero-arg method, `(T,)` for one param.
    fn payload_tuple_type(&self, method: &MethodDef) -> String {
        if method.params.is_empty() {
            return "()".to_string();
        }
        let parts: Vec<String> = method
            .params
            .iter()
            .map(|p| format_type_ref(&p.ty, &self.type_paths))
            .collect();
        if parts.len() == 1 {
            format!("({},)", parts[0])
        } else {
            format!("({})", parts.join(", "))
        }
    }

    /// Pattern that destructures the owned payload tuple back into locals named after the
    /// original params, for use inside the threadsafe-function's `build_callback` closure.
    fn payload_pattern(method: &MethodDef) -> String {
        if method.params.is_empty() {
            return "()".to_string();
        }
        let names: Vec<&str> = method.params.iter().map(|p| p.name.as_str()).collect();
        if names.len() == 1 {
            format!("({},)", names[0])
        } else {
            format!("({})", names.join(", "))
        }
    }

    /// Expression building the owned payload tuple from the trait method's own (possibly
    /// borrowed) parameters, evaluated on the calling thread before crossing to the
    /// threadsafe function — no napi handles are touched here.
    fn owned_payload_expr(&self, method: &MethodDef) -> String {
        if method.params.is_empty() {
            return "()".to_string();
        }
        let parts: Vec<String> = method.params.iter().map(|p| self.owned_param_expr(p)).collect();
        if parts.len() == 1 {
            format!("({},)", parts[0])
        } else {
            format!("({})", parts.join(", "))
        }
    }

    /// Whether `core::T` derives/implements `Copy` — a plain struct's `TypeDef::is_copy` or an
    /// enum's `EnumDef::is_copy`. `.clone()` on a `Copy` value is `clippy::clone_on_copy` (real
    /// lint on GENERATED code, `-D warnings` in the real gate) — checked so the fieldless-enum
    /// param shape (common: `Mode`-style options enums) doesn't trip it.
    fn is_copy_named(&self, name: &str) -> bool {
        self.api.types.iter().any(|t| t.name == name && t.is_copy)
            || self.api.enums.iter().any(|e| e.name == name && e.is_copy)
    }

    fn owned_param_expr(&self, p: &ParamDef) -> String {
        let name = &p.name;
        match &p.ty {
            TypeRef::String => {
                if p.is_ref {
                    format!("{name}.to_string()")
                } else {
                    format!("{name}.clone()")
                }
            }
            TypeRef::Bytes => {
                if p.is_ref {
                    format!("{name}.to_vec()")
                } else {
                    format!("{name}.clone()")
                }
            }
            TypeRef::Path => {
                if p.is_ref {
                    format!("{name}.to_path_buf()")
                } else {
                    format!("{name}.clone()")
                }
            }
            TypeRef::Primitive(_) | TypeRef::Char => {
                if p.is_ref {
                    format!("*{name}")
                } else {
                    name.to_string()
                }
            }
            TypeRef::Named(n) if self.is_copy_named(n) => {
                if p.is_ref {
                    format!("*{name}")
                } else {
                    name.to_string()
                }
            }
            _ => format!("{name}.clone()"),
        }
    }

    /// Args tuple type used by the threadsafe-function's JS-side call: the raw
    /// `napi::sys::napi_value` pointer, not `Unknown<'_>`/`Unknown<'static>`. A `build_callback`
    /// closure returning `(Unknown<'static>, ..)` -- the natural choice, since a module-level
    /// type alias needs a named lifetime -- fails to compile from inside an `async fn` under
    /// `#[async_trait]` with "implementation of `JsValue` is not general enough": the boxed
    /// future needs the closure's returned tuple to satisfy `JsValuesTupleIntoVec` for an
    /// arbitrary caller-supplied lifetime (`for<'a> JsValue<'a>`), but pinning `'static` commits
    /// to one specific lifetime instead. `napi_value` (a bare pointer, no lifetime parameter at
    /// all -- `ToNapiValue` is implemented for it as an identity conversion) sidesteps the
    /// variance question entirely. [`super::visitor_bridge::unknown_tuple_type`]'s `Unknown`
    /// form is unaffected and stays in use for the sync fast path / visitor path, neither of
    /// which crosses an `async fn` boundary.
    fn js_args_tuple_type_static(count: usize) -> String {
        if count == 0 {
            return "()".to_string();
        }
        let parts = vec!["napi::sys::napi_value"; count];
        format!("({}{})", parts.join(", "), if count == 1 { "," } else { "" })
    }

    /// Rewrite one `build_napi_args` expression (always ending in
    /// `Unknown::from_raw_unchecked(<env>.raw(), r)`) to yield the bare raw pointer `r` instead,
    /// matching [`Self::js_args_tuple_type_static`]'s `napi_value` element type. See that
    /// function's doc for why the TSFN closure needs this and the sync/visitor paths don't.
    fn to_raw_napi_value(expr: &str) -> String {
        // Every arm funnels through `visitor_bridge::to_napi_value_expr`/`napi_null_expr`, whose
        // text is exactly `unsafe { let r = ToNapiValue::to_napi_value(..).unwrap_or(..); \
        // Unknown::from_raw_unchecked(<env>.raw(), r) }` (after the `self.env()` -> `ctx.env`
        // substitution already applied). `to_napi_value(..).unwrap_or(..)` already IS the bare
        // `napi::sys::napi_value` this context needs -- drop the `let r = ..; Unknown::..(r)`
        // indirection entirely rather than keep it and collapse to `{ let r = X; r }`
        // (`clippy::let_and_return`). A literal replace, not a parser, but the text is generated
        // by code in this same crate (not user input), so its shape is a known, stable contract.
        expr.replace(
            "let r = napi::bindgen_prelude::ToNapiValue::to_napi_value",
            "napi::bindgen_prelude::ToNapiValue::to_napi_value",
        )
        .replace(
            "; napi::bindgen_prelude::Unknown::from_raw_unchecked(ctx.env.raw(), r) }",
            " }",
        )
    }

    /// The Rust plan for decoding a bridged method's non-`Unit` return type out of
    /// `AlefJsReply<T>`.
    fn plan_return_decode(&self, ty: &TypeRef) -> ReturnDecode {
        if is_napi_decodable(ty) {
            return ReturnDecode::Native {
                reply_ty: format_type_ref(ty, &self.type_paths),
            };
        }
        if let Some(f64_ty) = f32_bridge_target(ty) {
            return ReturnDecode::F32 {
                reply_ty: format_type_ref(&f64_ty, &self.type_paths),
                cast_expr: f32_bridge_cast_expr(ty, "__decoded"),
            };
        }
        match ty {
            TypeRef::Named(n) if self.is_native_return_type(n) => ReturnDecode::Named {
                reply_ty: self.js_dto_type(n),
                core_path: self.type_paths.get(n).cloned().unwrap_or_else(|| n.clone()),
            },
            TypeRef::Optional(inner) => {
                if let TypeRef::Named(n) = inner.as_ref()
                    && self.is_native_return_type(n)
                {
                    return ReturnDecode::Optional {
                        reply_ty: format!("Option<{}>", self.js_dto_type(n)),
                        core_path: self.type_paths.get(n).cloned().unwrap_or_else(|| n.clone()),
                    };
                }
                ReturnDecode::Json {
                    target_ty: format_type_ref(ty, &self.type_paths),
                }
            }
            TypeRef::Vec(inner) => {
                if let TypeRef::Named(n) = inner.as_ref()
                    && self.is_native_return_type(n)
                {
                    return ReturnDecode::VecNamed {
                        reply_ty: format!("Vec<{}>", self.js_dto_type(n)),
                        core_path: self.type_paths.get(n).cloned().unwrap_or_else(|| n.clone()),
                    };
                }
                ReturnDecode::Json {
                    target_ty: format_type_ref(ty, &self.type_paths),
                }
            }
            other => ReturnDecode::Json {
                target_ty: format_type_ref(other, &self.type_paths),
            },
        }
    }

    /// A `Unit`-returning method with exactly one `&mut` param of a native-marshalled struct
    /// type is the `PostProcessor::process`-shape write-back case: the host mutates its own copy
    /// and returns it (or nothing, to mean "unchanged"), instead of the bridge silently
    /// discarding whatever the JS side returned.
    fn writeback_param<'a>(&self, method: &'a MethodDef) -> Option<&'a ParamDef> {
        if !matches!(method.return_type, TypeRef::Unit) {
            return None;
        }
        method
            .params
            .iter()
            .find(|p| p.is_mut && matches!(&p.ty, TypeRef::Named(n) if self.struct_param_types.contains(n)))
    }
}

enum ReturnDecode {
    /// Already napi-native (String/bool/numeric/Vec of those) — no conversion beyond the decode.
    Native {
        reply_ty: String,
    },
    /// `f32`-only leaf: decode via the `f64` analog, then element-wise `as f32`.
    F32 {
        reply_ty: String,
        cast_expr: String,
    },
    /// A single native-marshalled-struct return: decode the `Js{T}` DTO, then `core::T::from`.
    Named {
        reply_ty: String,
        core_path: String,
    },
    Optional {
        reply_ty: String,
        core_path: String,
    },
    VecNamed {
        reply_ty: String,
        core_path: String,
    },
    /// Fallback: decode as `serde_json::Value`, then `serde_json::from_value::<T>`.
    Json {
        target_ty: String,
    },
}

impl ReturnDecode {
    fn reply_ty(&self) -> &str {
        match self {
            Self::Native { reply_ty } | Self::F32 { reply_ty, .. } | Self::Named { reply_ty, .. } => reply_ty,
            Self::Optional { reply_ty, .. } | Self::VecNamed { reply_ty, .. } => reply_ty,
            Self::Json { .. } => "serde_json::Value",
        }
    }

    /// Rust statement converting the settled `__decoded: {reply_ty}` local into `__result`, the
    /// method's real return value. `fail_stmt` is the statement to run on a decode failure (only
    /// reachable for the `Json` variant, the only fallible conversion).
    fn convert_stmt(&self, fail_stmt: &str) -> String {
        match self {
            Self::Native { .. } => "let __result = __decoded;".to_string(),
            Self::F32 { cast_expr, .. } => format!("let __result = {cast_expr};"),
            Self::Named { core_path, .. } => format!("let __result = {core_path}::from(__decoded);"),
            Self::Optional { core_path, .. } => {
                format!("let __result = __decoded.map({core_path}::from);")
            }
            Self::VecNamed { core_path, .. } => {
                format!("let __result = __decoded.into_iter().map({core_path}::from).collect();")
            }
            Self::Json { target_ty } => format!(
                "let __result = match serde_json::from_value::<{target_ty}>(__decoded) {{ \
                 Ok(v) => v, \
                 Err(e) => {{ {fail_stmt} }} \
                 }};"
            ),
        }
    }
}

impl TraitBridgeGenerator for NapiBridgeGenerator {
    fn foreign_object_type(&self) -> &str {
        "napi::bindgen_prelude::Object<'static>"
    }

    fn gen_method_presence_check(&self, method: &MethodDef, _spec: &TraitBridgeSpec) -> Option<String> {
        self.forwardable_defaulted
            .contains(&method.name)
            .then(|| format!("self.{}.is_some()", Self::tsfn_field_name(method)))
    }

    fn extra_bridge_fields(&self, spec: &TraitBridgeSpec) -> Vec<(String, String)> {
        self.tsfn_methods(spec)
            .into_iter()
            .map(|m| {
                let alias = self.tsfn_alias_name(spec, &m);
                let ty = if self.is_forwardable(&m) {
                    format!("Option<{alias}>")
                } else {
                    alias
                };
                (Self::tsfn_field_name(&m), ty)
            })
            .collect()
    }

    fn bridge_imports(&self) -> Vec<String> {
        vec![
            "napi::bindgen_prelude::{JsObjectValue, ToNapiValue, Unknown, Object}".to_string(),
            "napi::JsValue".to_string(),
            "std::sync::Arc".to_string(),
        ]
    }

    /// Synchronous trait methods use a fast/slow split rather than always paying a
    /// threadsafe-function round trip: `new()` (and therefore every non-async caller reachable
    /// from the `#[napi] register_*` frame, including a probe that calls the bridged method
    /// immediately after construction — see `TokenizerBackendRegistry::register`) runs on the JS
    /// thread, so the overwhelming common case can call the JS method directly. A caller on any
    /// other thread (e.g. `TokenizerBackendSizer::size` running inside the chunk splitter) cannot
    /// safely do that — `self.env()`/`self.obj(&env)` need a live `HandleScope`, which only
    /// exists on the JS thread — so it goes through the method's `ThreadsafeFunction` instead,
    /// blocking synchronously on a bounded channel.
    fn gen_sync_method_body(&self, method: &MethodDef, spec: &TraitBridgeSpec) -> String {
        let wrapper = spec.wrapper_name();
        let has_error = method.error_type.is_some();
        let on_js_thread_name = format!("{}_on_js_thread", method.name);
        let call_args = param_call_args(method);

        let fast_path_body = self.gen_sync_on_js_thread_body(method, spec);
        self.extra_impl_items.borrow_mut().push(format!(
            "impl {wrapper} {{\n    fn {on_js_thread_name}(&self, {params}) -> {ret} {{\n{body}\n    }}\n}}",
            params = sync_inherent_params(method, &self.type_paths),
            ret = sync_return_type(method, has_error, &self.error_path(spec), &self.type_paths),
            body = indent(&fast_path_body, 2),
        ));

        let slow_path_body = self.gen_sync_slow_path_body(method, spec);

        format!(
            "if std::thread::current().id() == self.js_thread {{\n    return self.{on_js_thread_name}({call_args});\n}}\n{slow_path_body}"
        )
    }

    /// Every bridged async method is dispatched through its `ThreadsafeFunction`:
    /// `call_async_catch` (never `call_async`, whose JS-throw path routes through
    /// `napi_fatal_exception` and kills the host process) enqueues the call and awaits its
    /// result off-thread, then `AlefJsReply::settle` awaits the `Promise` if the host returned
    /// one. Both steps preserve the underlying error's `Display` text in the mapped
    /// `core::Error` rather than discarding it.
    fn gen_async_method_body(&self, method: &MethodDef, spec: &TraitBridgeSpec) -> String {
        let tsfn_field = Self::tsfn_field_name(method);
        let payload_expr = self.owned_payload_expr(method);
        let has_default_impl = method.has_default_impl && self.is_forwardable(method);
        let has_error = method.error_type.is_some();
        let wrapper = spec.wrapper_name();

        let error_call = spec.make_error(&format!(
            "format!(\"Plugin '{{}}' method '{}' failed: {{}}\", self.cached_name, e)",
            method.name
        ));
        let error_settle = spec.make_error(&format!(
            "format!(\"Plugin '{{}}' method '{}' rejected: {{}}\", self.cached_name, e)",
            method.name
        ));
        let error_parse = spec.make_error(&format!(
            "format!(\"Plugin '{{}}' failed to parse return value for method '{}': {{}}\", self.cached_name, e)",
            method.name
        ));

        // Unwrap a `napi::Result<T>`-typed expression into `T`: propagate via `?` for a
        // fallible trait method, or log-and-substitute a default for an infallible one (an
        // async trait method's `Err` has nowhere else to go — the signature has no `Result`).
        let unwrap = |expr: &str, error_expr: &str, step: &str| -> String {
            if has_error {
                format!("({expr}).map_err(|e| {error_expr})?")
            } else {
                format!(
                    "match {expr} {{\n    \
                     Ok(v) => v,\n    \
                     Err(e) => {{\n        \
                     tracing::warn!(wrapper = \"{wrapper}\", method = \"{method_name}\", error = %e, \"{step}; returning default\");\n        \
                     return Default::default();\n    \
                     }}\n\
                     }}",
                    method_name = method.name,
                )
            }
        };

        // `self.<field>` is not `Copy` (`ThreadsafeFunction` isn't), so a required method (plain
        // field) must borrow it explicitly (`&self.field`) rather than move out of `&self` --
        // `.as_ref()` on the `Option<..>` forwardable case already produces a borrow on its own.
        let tsfn_expr = if has_default_impl {
            format!("self.{tsfn_field}.as_ref().expect(\"presence-checked by the default-method guard\")")
        } else {
            format!("&self.{tsfn_field}")
        };
        let call_prelude = format!("let __payload = {payload_expr};\nlet __call = {tsfn_expr};");

        if let Some(mut_param) = self.writeback_param(method) {
            let TypeRef::Named(core_name) = &mut_param.ty else {
                unreachable!("writeback_param only returns Named params");
            };
            let js_dto = self.js_dto_type(core_name);
            let core_path = self
                .type_paths
                .get(core_name.as_str())
                .cloned()
                .unwrap_or_else(|| core_name.clone());
            let reply = unwrap(
                "__call.call_async_catch(__payload).await",
                &error_call,
                "threadsafe call failed",
            );
            let settled = unwrap("__reply.settle().await", &error_settle, "host callback rejected");
            let tail = if has_error { "Ok(())" } else { "()" };
            return format!(
                "{call_prelude}\n\
                 let __reply: AlefJsReply<Option<{js_dto}>> = {reply};\n\
                 let __maybe: Option<{js_dto}> = {settled};\n\
                 if let Some(__js_result) = __maybe {{\n    *{mut_name} = {core_path}::from(__js_result);\n}}\n\
                 {tail}",
                mut_name = mut_param.name,
            );
        }

        if matches!(method.return_type, TypeRef::Unit) {
            let reply = unwrap(
                "__call.call_async_catch(__payload).await",
                &error_call,
                "threadsafe call failed",
            );
            let settled = unwrap("__reply.settle().await", &error_settle, "host callback rejected");
            let tail = if has_error { format!("Ok({settled})") } else { settled };
            return format!(
                "{call_prelude}\n\
                 let __reply: AlefJsReply<()> = {reply};\n\
                 {tail}"
            );
        }

        let decode = self.plan_return_decode(&method.return_type);
        let reply_ty = decode.reply_ty();
        let fail_stmt = if has_error {
            format!("return Err({error_parse});")
        } else {
            format!(
                "tracing::warn!(wrapper = \"{wrapper}\", method = \"{method_name}\", error = %e, \"host returned an unparseable value; returning default\");\n    return Default::default();",
                method_name = method.name,
            )
        };
        // `ReturnDecode::Native` is a pure passthrough (`__decoded` already IS the return type).
        // An infallible method's tail is bare (`__decoded`, no `Ok(..)` wrapper), so binding
        // `settled` to `__decoded` first and using it as the very next tail is exactly
        // `clippy::let_and_return` (a real lint on the GENERATED code, caught by a scratch-crate
        // `cargo clippy` compile of this shape -- `-D warnings` in the real gate): inline
        // `settled` directly as the tail instead. The fallible case's tail is `Ok(__decoded)`,
        // not bare `__decoded`, so clippy does not flag it -- keep the `let` there for
        // readability (a multi-line `match` inlined into `Ok(..)` reads worse than named).
        let is_identity = matches!(decode, ReturnDecode::Native { .. });
        let reply = unwrap(
            "__call.call_async_catch(__payload).await",
            &error_call,
            "threadsafe call failed",
        );
        let settled = unwrap("__reply.settle().await", &error_settle, "host callback rejected");

        if is_identity && !has_error {
            return format!(
                "{call_prelude}\n\
                 let __reply: AlefJsReply<{reply_ty}> = {reply};\n\
                 {settled}"
            );
        }

        let convert = if is_identity {
            String::new()
        } else {
            decode.convert_stmt(&fail_stmt)
        };
        let result_var = if is_identity { "__decoded" } else { "__result" };
        let tail = if has_error {
            format!("Ok({result_var})")
        } else {
            result_var.to_string()
        };
        format!(
            "{call_prelude}\n\
             let __reply: AlefJsReply<{reply_ty}> = {reply};\n\
             let __decoded: {reply_ty} = {settled};\n\
             {convert}\n\
             {tail}"
        )
    }

    fn gen_constructor(&self, spec: &TraitBridgeSpec) -> String {
        let wrapper = spec.wrapper_name();
        let required_methods = spec
            .required_methods()
            .iter()
            .map(|m| {
                let js_name = to_camel_case(&m.name);
                let snake_name = m.name.clone();
                minijinja::context! {
                    name => js_name,
                    snake_case_name => snake_name,
                }
            })
            .collect::<Vec<_>>();

        let tsfn_field_inits: Vec<String> = self
            .tsfn_methods(spec)
            .into_iter()
            .map(|m| self.gen_tsfn_field_init(spec, &m))
            .collect();

        crate::backends::napi::template_env::render(
            "trait_bridge_constructor.jinja",
            minijinja::context! {
                wrapper_name => wrapper,
                required_methods => required_methods,
                requires_plugin_name => spec.bridge_config.super_trait.is_some(),
                tsfn_field_inits => tsfn_field_inits,
            },
        )
    }

    fn gen_unregistration_fn(&self, spec: &TraitBridgeSpec) -> String {
        let Some(unregister_fn) = spec.bridge_config.unregister_fn.as_deref() else {
            return String::new();
        };
        let host_path = host_function_path(spec, unregister_fn);
        let camel = to_camel_case(unregister_fn);
        crate::backends::napi::template_env::render(
            "unregistration_fn.jinja",
            minijinja::context! {
                unregister_fn => unregister_fn,
                camel_fn_name => camel,
                host_path => host_path,
            },
        )
    }

    fn gen_clear_fn(&self, spec: &TraitBridgeSpec) -> String {
        let Some(clear_fn) = spec.bridge_config.clear_fn.as_deref() else {
            return String::new();
        };
        let host_path = host_function_path(spec, clear_fn);
        let camel = to_camel_case(clear_fn);
        crate::backends::napi::template_env::render(
            "clear_fn.jinja",
            minijinja::context! {
                clear_fn => clear_fn,
                camel_fn_name => camel,
                host_path => host_path,
            },
        )
    }

    fn gen_registration_fn(&self, spec: &TraitBridgeSpec) -> String {
        let Some(register_fn) = spec.bridge_config.register_fn.as_deref() else {
            return String::new();
        };
        let Some(registry_getter) = spec.bridge_config.registry_getter.as_deref() else {
            return String::new();
        };
        let wrapper = spec.wrapper_name();
        let trait_path = spec.trait_path();

        let extra = spec
            .bridge_config
            .register_extra_args
            .as_deref()
            .map(|a| format!(", {a}"))
            .unwrap_or_default();

        crate::backends::napi::template_env::render(
            "registration_fn.jinja",
            minijinja::context! {
                register_fn => register_fn,
                wrapper => wrapper,
                trait_path => trait_path,
                registry_getter => registry_getter,
                extra_args => extra,
            },
        )
    }
}

impl NapiBridgeGenerator {
    fn error_path(&self, spec: &TraitBridgeSpec) -> String {
        spec.error_path()
    }

    /// Body of the `<method>_on_js_thread` private inherent method: today's direct-call shape,
    /// unchanged — `self.env()`/`self.obj(&env)`/`func.call(..)` are safe here because this path
    /// only ever runs on the JS thread (checked by the caller in
    /// [`TraitBridgeGenerator::gen_sync_method_body`]).
    fn gen_sync_on_js_thread_body(&self, method: &MethodDef, spec: &TraitBridgeSpec) -> String {
        let js_method_name = to_camel_case(&method.name);
        let snake_method_name = method.name.clone();
        let has_error = method.error_type.is_some();

        let js_args_exprs = build_napi_args(
            method,
            spec.bridge_config,
            &self.struct_param_types,
            &self.type_prefix,
            &spec.trait_def.name,
            &self.unsupported_args,
            &self.json_safe_named_types(),
        );
        let (args_tuple_ty, empty_args, tuple_args) = args_tuple_pieces(&js_args_exprs);

        let error_lookup =
            spec.make_error("format!(\"Method '{}' not found on bridge object: {}\", self.cached_name, e)");
        let error_call = spec.make_error(&format!(
            "format!(\"Plugin '{{}}' method '{}' failed: {{}}\", self.cached_name, e)",
            method.name
        ));

        let has_default_impl = method.has_default_impl;
        if matches!(method.return_type, TypeRef::Unit) {
            crate::backends::napi::template_env::render(
                "sync_method_unit_return.jinja",
                minijinja::context! {
                    wrapper => spec.wrapper_name(),
                    method_name => &js_method_name,
                    snake_case_method_name => &snake_method_name,
                    args_tuple_ty => args_tuple_ty,
                    has_error => has_error,
                    has_default_impl => has_default_impl,
                    empty_args => empty_args,
                    tuple_args => tuple_args,
                    error_lookup => error_lookup,
                    error_call => error_call,
                },
            )
        } else {
            let error_coercion = spec.make_error(&format!(
                "format!(\"Failed to extract return value from method '{}': {{}}\", e)",
                method.name
            ));
            let error_parse = spec.make_error(&format!(
                "format!(\"Plugin '{{}}' failed to parse return value for method '{}'\", self.cached_name)",
                method.name
            ));
            crate::backends::napi::template_env::render(
                "sync_method_non_unit_return.jinja",
                minijinja::context! {
                    wrapper => spec.wrapper_name(),
                    method_name => &js_method_name,
                    snake_case_method_name => &snake_method_name,
                    args_tuple_ty => args_tuple_ty,
                    has_error => has_error,
                    has_default_impl => has_default_impl,
                    empty_args => empty_args,
                    tuple_args => tuple_args,
                    error_lookup => error_lookup,
                    error_call => error_call,
                    error_coercion => error_coercion,
                    error_parse => error_parse,
                },
            )
        }
    }

    /// Body of the trait-impl method's off-JS-thread slow path: block synchronously on the
    /// method's threadsafe function via a bounded `mpsc` channel, re-checking `aborted()` on
    /// each wake so environment teardown surfaces as an error instead of hanging forever.
    fn gen_sync_slow_path_body(&self, method: &MethodDef, spec: &TraitBridgeSpec) -> String {
        let tsfn_field = Self::tsfn_field_name(method);
        let has_error = method.error_type.is_some();
        let has_default_impl = method.has_default_impl && self.is_forwardable(method);
        let payload_expr = self.owned_payload_expr(method);

        let infallible_default = "Default::default()";
        // The trait-impl method is only ever entered here once the caller-visible presence
        // check has already passed: `gen_method_presence_check` (`self.<m>_tsfn.is_some()`)
        // wraps every forwardable method in a guard that returns the Rust-default delegate
        // BEFORE this body runs (see `default_method_guard.jinja` / `gen_bridge_trait_impl`), so
        // by the time control reaches here the `Option` is always `Some`. Unwrapping it a second
        // time with its own `Default::default()` fallback would not even type-check for a
        // fallible method (`Result<T, E>` has no `Default` impl) -- `.expect(..)` documents the
        // real invariant instead of re-deriving a (wrong) one.
        let missing_tsfn_arm = if has_default_impl {
            format!("let __tsfn = self.{tsfn_field}.as_ref().expect(\"presence-checked by the default-method guard\");")
        } else {
            format!("let __tsfn = &self.{tsfn_field};")
        };

        // The channel carries the RAW decode target (`ReturnDecode::reply_ty`, e.g. `JsReport` or
        // `serde_json::Value`), not the method's real return type -- `Ok(v) => v` on its own
        // (the shape this block had before) skipped `ReturnDecode::convert_stmt` entirely and
        // handed back the wrong type for every `Named`/`Optional`/`VecNamed`/`Json` return
        // (caught by a scratch-crate compile of the "enum return" fixture shape: `Ok(v) => v`
        // returned `serde_json::Value` where the method's signature said `Mode`). Every arm that
        // yields a settled value now goes through the same conversion the async path uses.
        let decode = if matches!(method.return_type, TypeRef::Unit) {
            None
        } else {
            Some(self.plan_return_decode(&method.return_type))
        };
        let reply_ty = decode
            .as_ref()
            .map(|d| d.reply_ty().to_string())
            .unwrap_or_else(|| "()".to_string());

        let error_parse = spec.make_error(&format!(
            "format!(\"Plugin '{{}}' failed to parse return value for method '{}': {{}}\", self.cached_name, e)",
            method.name
        ));
        let convert_fail_stmt = if has_error {
            format!("return Err({error_parse});")
        } else {
            format!(
                "tracing::warn!(wrapper = \"{}\", method = \"{}\", error = %e, \"host returned an unparseable value; returning default\");\n    return {infallible_default};",
                spec.wrapper_name(),
                method.name
            )
        };
        // `Ok(v) => { <convert> <tail> }` — `convert` is `""` (identity) for a `Unit` return OR a
        // `ReturnDecode::Native` return (`__decoded` already IS the return type; see the doc on
        // the matching special case in `gen_async_method_body`, which explains why the redundant
        // `let __result = __decoded;` immediately before a bare `__result` tail must be skipped
        // rather than emitted -- `clippy::let_and_return` under the real gate's `-D warnings`).
        let convert_and_tail = |ok_prefix: &str, ok_suffix: &str| -> String {
            match &decode {
                None => format!("{ok_prefix}v{ok_suffix}"),
                Some(ReturnDecode::Native { .. }) => {
                    format!("{ok_prefix}v{ok_suffix}")
                }
                Some(d) => format!(
                    "{{ let __decoded = v; {convert} {ok_prefix}__result{ok_suffix} }}",
                    convert = d.convert_stmt(&convert_fail_stmt),
                ),
            }
        };

        let error_settle = spec.make_error(&format!(
            "format!(\"Plugin '{{}}' method '{}' rejected: {{}}\", self.cached_name, e)",
            method.name
        ));

        let recv_body = if has_error {
            format!(
                "let __settled: napi::Result<{reply_ty}> = match __rx.recv_timeout(std::time::Duration::from_millis(50)) {{\n\
                 \x20   Ok(v) => v,\n\
                 \x20   Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {{\n\
                 \x20       if __tsfn.aborted() {{\n\
                 \x20           return Err({error_call_timeout});\n\
                 \x20       }}\n\
                 \x20       continue;\n\
                 \x20   }}\n\
                 \x20   Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err({error_disconnected}),\n\
                 }};\n\
                 return match __settled {{\n\
                 \x20   Ok(v) => {ok_arm},\n\
                 \x20   Err(e) => Err({error_settle}),\n\
                 }};",
                error_call_timeout = spec.make_error(&format!(
                    "format!(\"Plugin '{{}}' method '{}' timed out\", self.cached_name)",
                    method.name
                )),
                error_disconnected = spec.make_error(&format!(
                    "format!(\"Plugin '{{}}' method '{}' channel disconnected before a reply arrived\", self.cached_name)",
                    method.name
                )),
                ok_arm = convert_and_tail("Ok(", ")"),
            )
        } else {
            format!(
                "let __settled: napi::Result<{reply_ty}> = match __rx.recv_timeout(std::time::Duration::from_millis(50)) {{\n\
                 \x20   Ok(v) => v,\n\
                 \x20   Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {{\n\
                 \x20       if __tsfn.aborted() {{\n\
                 \x20           tracing::warn!(wrapper = \"{wrapper}\", method = \"{method_name}\", \"threadsafe call timed out; returning default\");\n\
                 \x20           return {infallible_default};\n\
                 \x20       }}\n\
                 \x20       continue;\n\
                 \x20   }}\n\
                 \x20   Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {{\n\
                 \x20       tracing::warn!(wrapper = \"{wrapper}\", method = \"{method_name}\", \"threadsafe call channel disconnected; returning default\");\n\
                 \x20       return {infallible_default};\n\
                 \x20   }}\n\
                 }};\n\
                 return match __settled {{\n\
                 \x20   Ok(v) => {ok_arm},\n\
                 \x20   Err(e) => {{\n\
                 \x20       tracing::warn!(wrapper = \"{wrapper}\", method = \"{method_name}\", error = %e, \"host callback failed; returning default\");\n\
                 \x20       {infallible_default}\n\
                 \x20   }}\n\
                 }};",
                wrapper = spec.wrapper_name(),
                method_name = method.name,
                ok_arm = convert_and_tail("", ""),
            )
        };

        format!(
            "{missing_tsfn_arm}\n\
             let __payload = {payload_expr};\n\
             let __plugin_name = self.cached_name.clone();\n\
             let (__tx, __rx) = std::sync::mpsc::sync_channel::<napi::Result<{reply_ty}>>(1);\n\
             let __enqueue_status = __tsfn.call_with_return_value(\n\
             \x20   __payload,\n\
             \x20   napi::threadsafe_function::ThreadsafeFunctionCallMode::NonBlocking,\n\
             \x20   move |reply: napi::Result<AlefJsReply<{reply_ty}>>, _env: napi::Env| {{\n\
             \x20       // Declared synchronous, so there is no executor here to await a Promise: a host\n\
             \x20       // that returns one anyway is a protocol error (`settle_sync` reports it as such)\n\
             \x20       // rather than being polled once and misread as immediately ready.\n\
             \x20       let settled = reply.and_then(|r| r.settle_sync(&__plugin_name, \"{method_name}\"));\n\
             \x20       let _ = __tx.send(settled);\n\
             \x20       Ok(())\n\
             \x20   }},\n\
             );\n\
             if __enqueue_status != napi::Status::Ok {{\n\
             \x20   {enqueue_fail}\n\
             }}\n\
             loop {{\n{recv_body}\n}}",
            method_name = method.name,
            enqueue_fail = if has_error {
                format!("return Err({});", spec.make_error(&format!(
                    "format!(\"Plugin '{{}}' method '{}' could not be enqueued: {{:?}}\", self.cached_name, __enqueue_status)",
                    method.name
                )))
            } else {
                format!(
                    "tracing::warn!(wrapper = \"{}\", method = \"{}\", status = ?__enqueue_status, \"failed to enqueue threadsafe call; returning default\");\n    return {infallible_default};",
                    spec.wrapper_name(),
                    method.name
                )
            }
        )
    }

    /// Emit the `Self { field: <init> }` initializer for one method's `ThreadsafeFunction`
    /// field, built eagerly in `new()` — which already runs on the JS thread inside the
    /// synchronous `#[napi] register_*` frame, with a live `Object` (`js_obj`) in scope.
    fn gen_tsfn_field_init(&self, spec: &TraitBridgeSpec, method: &MethodDef) -> String {
        let field = Self::tsfn_field_name(method);
        let alias = self.tsfn_alias_name(spec, method);
        let js_name = to_camel_case(&method.name);
        let snake_name = &method.name;
        let payload_ty = self.payload_tuple_type(method);
        let payload_pattern = Self::payload_pattern(method);

        let is_writeback = self.writeback_param(method).is_some();
        let reply_ty = if is_writeback {
            let mut_param = self.writeback_param(method).expect("checked above");
            let TypeRef::Named(core_name) = &mut_param.ty else {
                unreachable!("writeback_param only returns Named params");
            };
            format!("Option<{}>", self.js_dto_type(core_name))
        } else if matches!(method.return_type, TypeRef::Unit) {
            "()".to_string()
        } else {
            self.plan_return_decode(&method.return_type).reply_ty().to_string()
        };

        // Every param is already owned by the time it's destructured out of the payload tuple
        // inside the closure below (`Self::owned_payload_expr` stripped the reference on the
        // calling thread) -- force `is_ref: false` so `build_napi_args`'s arms emit the owned-
        // value form (`x.clone()`) instead of the reference form (`(*x).clone()`, which would
        // try to deref an already-owned value and fail to compile).
        let mut owned_method = method.clone();
        for p in &mut owned_method.params {
            p.is_ref = false;
        }
        let js_args_exprs = build_napi_args(
            &owned_method,
            spec.bridge_config,
            &self.struct_param_types,
            &self.type_prefix,
            &spec.trait_def.name,
            &self.unsupported_args,
            &self.json_safe_named_types(),
        )
        .into_iter()
        .map(|expr| Self::to_raw_napi_value(&expr.replace("self.env()", "ctx.env")))
        .collect::<Vec<_>>();
        let arg_count = js_args_exprs.len();
        let args_tuple_expr = if arg_count == 0 {
            "()".to_string()
        } else if arg_count == 1 {
            format!("({},)", js_args_exprs[0])
        } else {
            format!("({})", js_args_exprs.join(", "))
        };

        let build_expr = format!(
            "{{\n\
             \x20   let __f: napi::bindgen_prelude::Function<'_, (), AlefJsReply<{reply_ty}>> = js_obj\n\
             \x20       .get_named_property(\"{js_name}\")\n\
             \x20       .or_else(|_| js_obj.get_named_property(\"{snake_name}\"))?;\n\
             \x20   let __built: {alias} = __f\n\
             \x20       .build_threadsafe_function::<{payload_ty}>()\n\
             \x20       .weak::<true>()\n\
             \x20       .max_queue_size::<0>()\n\
             \x20       .callee_handled::<false>()\n\
             \x20       .build_callback(move |ctx: napi::threadsafe_function::ThreadsafeCallContext<{payload_ty}>| {{\n\
             \x20           let {payload_pattern} = ctx.value;\n\
             \x20           Ok({args_tuple_expr})\n\
             \x20       }})?;\n\
             \x20   __built\n\
             }}"
        );

        if self.is_forwardable(method) {
            format!(
                "{field}: if js_obj.has_named_property(\"{js_name}\").unwrap_or(false)\n\
                 \x20   || js_obj.has_named_property(\"{snake_name}\").unwrap_or(false)\n\
                 {{\n    Some({build_expr})\n}} else {{\n    None\n}},"
            )
        } else {
            format!("{field}: {build_expr},")
        }
    }

    /// Emit the module-level `type <Wrapper><Method>Tsfn = ThreadsafeFunction<..>;` alias for
    /// every bridged method, placed just before the wrapper struct so the struct's own field
    /// declarations (built via [`Self::extra_bridge_fields`]) can name them.
    pub(super) fn gen_tsfn_aliases(&self, spec: &TraitBridgeSpec) -> String {
        self.tsfn_methods(spec)
            .into_iter()
            .map(|m| {
                let m = &m;
                let alias = self.tsfn_alias_name(spec, m);
                let payload_ty = self.payload_tuple_type(m);
                let reply_ty = if let Some(mut_param) = self.writeback_param(m) {
                    let TypeRef::Named(core_name) = &mut_param.ty else {
                        unreachable!("writeback_param only returns Named params");
                    };
                    format!("Option<{}>", self.js_dto_type(core_name))
                } else if matches!(m.return_type, TypeRef::Unit) {
                    "()".to_string()
                } else {
                    self.plan_return_decode(&m.return_type).reply_ty().to_string()
                };
                let args_ty = Self::js_args_tuple_type_static(m.params.len());
                format!(
                    "type {alias} = napi::threadsafe_function::ThreadsafeFunction<\n    \
                     {payload_ty},\n    \
                     AlefJsReply<{reply_ty}>,\n    \
                     {args_ty},\n    \
                     napi::Status,\n    \
                     false,\n    \
                     true,\n    \
                     0,\n\
                     >;"
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Every private inherent-method item accumulated by [`TraitBridgeGenerator::gen_sync_method_body`]
    /// while generating the trait impl — the `<method>_on_js_thread` fast-path bodies. Drained
    /// (not cloned) since `gen_trait_bridge` calls this exactly once, after the trait impl is
    /// fully generated.
    pub(super) fn take_extra_impl_items(&self) -> Vec<String> {
        std::mem::take(&mut self.extra_impl_items.borrow_mut())
    }

    /// Sites where a parameter could not be marshalled to JS natively — see
    /// [`UnsupportedArg`]. `gen_trait_bridge` calls this once, after the whole bridge (including
    /// the visitor path's own params, which do not flow through this generator) is generated.
    pub(super) fn take_unsupported_args(&self) -> Vec<UnsupportedArg> {
        std::mem::take(&mut self.unsupported_args.borrow_mut())
    }
}

/// Args tuple pieces shared by both the fast-path sync body and (indirectly) the callback
/// builder: the `FnArgs<..>` type string, whether there are zero args, and the runtime tuple
/// expression.
fn args_tuple_pieces(js_args_exprs: &[String]) -> (String, bool, String) {
    let inner_tuple_ty = super::visitor_bridge::unknown_tuple_type(js_args_exprs.len());
    let empty_args = js_args_exprs.is_empty();
    let args_tuple_ty = if empty_args {
        inner_tuple_ty
    } else {
        format!("napi::bindgen_prelude::FnArgs<{inner_tuple_ty}>")
    };
    let tuple_args = if empty_args {
        String::new()
    } else if js_args_exprs.len() == 1 {
        format!("({},)", js_args_exprs[0])
    } else {
        format!("({})", js_args_exprs.join(", "))
    };
    (args_tuple_ty, empty_args, tuple_args)
}

/// Parameter list for the `<method>_on_js_thread` private inherent method — mirrors the trait
/// method's own parameter signature so the trait-impl fast path can forward every argument
/// verbatim.
fn sync_inherent_params(method: &MethodDef, type_paths: &HashMap<String, String>) -> String {
    method
        .params
        .iter()
        .map(|p| {
            format!(
                "{}: {}",
                p.name,
                crate::codegen::generators::trait_bridge::format_param_type(p, type_paths)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn param_call_args(method: &MethodDef) -> String {
    method
        .params
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

fn sync_return_type(
    method: &MethodDef,
    has_error: bool,
    error_path: &str,
    type_paths: &HashMap<String, String>,
) -> String {
    let base = crate::codegen::generators::trait_bridge::format_type_ref(&method.return_type, type_paths);
    if has_error {
        format!("std::result::Result<{base}, {error_path}>")
    } else {
        base
    }
}

/// Indent every line of `text` by `levels * 4` spaces.
fn indent(text: &str, levels: usize) -> String {
    let pad = "    ".repeat(levels);
    text.lines()
        .map(|line| format!("{pad}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
