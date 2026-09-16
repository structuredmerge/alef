use super::{
    TraitBridgeGenerator, TraitBridgeSpec, default_delegate_name, forwarded_defaulted_methods, trait_method_signature,
};

pub fn gen_bridge_trait_impl(spec: &TraitBridgeSpec, generator: &dyn TraitBridgeGenerator) -> String {
    let wrapper = spec.wrapper_name();
    let trait_path = spec.trait_path();

    let forwarded = forwarded_defaulted_methods(spec, generator);
    let own_methods: Vec<_> = spec
        .trait_def
        .methods
        .iter()
        .filter(|m| m.trait_source.is_none() && (!m.has_default_impl || forwarded.iter().any(|f| f.name == m.name)))
        .collect();

    let has_async_methods = own_methods.iter().any(|m| m.is_async);
    let async_trait_is_send = generator.async_trait_is_send();

    let mut methods_code = String::with_capacity(1024);
    for (i, method) in own_methods.iter().enumerate() {
        if i > 0 {
            methods_code.push_str("\n\n");
        }

        let sig = trait_method_signature(method, spec);
        let (async_kw, all_params, ret) = (sig.async_kw, sig.all_params, sig.ret);

        let host_body = if method.is_async {
            generator.gen_async_method_body(method, spec)
        } else {
            generator.gen_sync_method_body(method, spec)
        };

        let raw_body = match generator
            .gen_method_presence_check(method, spec)
            .filter(|_| method.has_default_impl)
        {
            Some(presence) => {
                let guard = crate::codegen::template_env::render(
                    "generators/trait_bridge/default_method_guard.jinja",
                    minijinja::context! {
                        presence => presence,
                        delegate_name => default_delegate_name(spec, method),
                        method_name => &method.name,
                        arg_names => &sig.arg_names,
                        is_async => method.is_async,
                    },
                );
                format!("{guard}{host_body}")
            }
            None => host_body,
        };

        let raw_body_trimmed = raw_body.trim();
        let body_is_static_slice = raw_body_trimmed.starts_with("self.") && raw_body_trimmed.ends_with("_strs");
        let returns_ref_string_vec = matches!(
            &method.return_type,
            crate::core::ir::TypeRef::Vec(inner) if matches!(inner.as_ref(), crate::core::ir::TypeRef::String)
        );
        let body = if method.returns_ref && returns_ref_string_vec {
            if body_is_static_slice {
                raw_body
            } else {
                // ~keep A body containing `return` is statement-shaped, not an expression: a
                // plain `{ ... }` block would let those `return`s escape the trait method with
                // the unconverted `Vec<String>`, which fails to typecheck against `&[&str]` and
                // leaves the conversion below unreachable (the napi sync bridge returns from
                // inside its recv_timeout loop). Such a body is collected through a closure so
                // every exit path reaches the conversion. An expression-shaped body keeps the
                // plain block -- wrapping it too would be a redundant closure call, which clippy
                // denies (the pyo3 bridge emits one bare `Python::attach(..)` expression).
                let needs_capture = contains_return_keyword(&raw_body);
                let collect_types = match (needs_capture, method.is_async) {
                    (false, _) => format!("let __types: Vec<String> = {{ {raw_body} }};"),
                    (true, true) => format!("let __types: Vec<String> = async {{ {raw_body} }}.await;"),
                    (true, false) => {
                        format!("let __types: Vec<String> = (|| -> Vec<String> {{ {raw_body} }})();")
                    }
                };
                format!(
                    "{collect_types}\n\
                     let __strs: Vec<&'static str> = __types.into_iter()\n\
                         .map(|s| -> &'static str {{ Box::leak(s.into_boxed_str()) }})\n\
                         .collect();\n\
                     Box::leak(__strs.into_boxed_slice())"
                )
            }
        } else {
            raw_body
        };

        let indented_body = body
            .lines()
            .map(|line| format!("        {line}"))
            .collect::<Vec<_>>()
            .join("\n");

        methods_code.push_str(&crate::codegen::template_env::render(
            "generators/trait_bridge/trait_method.jinja",
            minijinja::context! {
                async_kw => async_kw,
                method_name => &method.name,
                all_params => all_params,
                ret => ret,
                indented_body => &indented_body,
            },
        ));
    }

    crate::codegen::template_env::render(
        "generators/trait_bridge/trait_impl.jinja",
        minijinja::context! {
            has_async_methods => has_async_methods,
            async_trait_is_send => async_trait_is_send,
            trait_path => trait_path,
            wrapper_name => wrapper,
            methods_code => methods_code,
        },
    )
}

/// Whether `body` contains a `return` *keyword* rather than a substring such as `returning`,
/// which appears in this generator's own emitted log messages. Distinguishes a statement-shaped
/// emitted body from an expression-shaped one.
fn contains_return_keyword(body: &str) -> bool {
    body.match_indices("return").any(|(index, matched)| {
        let before_is_word = body[..index]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after_is_word = body[index + matched.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        !before_is_word && !after_is_word
    })
}
