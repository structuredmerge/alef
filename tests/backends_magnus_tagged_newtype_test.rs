use alef::backends::magnus::MagnusBackend;
use alef::core::backend::Backend;
use alef::core::config::new_config::NewAlefConfig;
use alef::core::ir::*;

fn generated_files(
    tag: Option<&str>,
    content: Option<&str>,
    untagged: bool,
    tuple: bool,
) -> Vec<alef::core::backend::GeneratedFile> {
    let (api, crate_config) = fixture_api(tag, content, untagged, tuple);
    MagnusBackend.generate_bindings(&api, &crate_config).unwrap()
}

fn fixture_api(
    tag: Option<&str>,
    content: Option<&str>,
    untagged: bool,
    tuple: bool,
) -> (ApiSurface, alef::core::config::ResolvedCrateConfig) {
    let config: NewAlefConfig = toml::from_str(
        r#"
[workspace]
languages = ["ruby"]
[[crates]]
name = "fixture"
sources = ["src/lib.rs"]
"#,
    )
    .unwrap();
    let api = ApiSurface {
        crate_name: "fixture".into(),
        enums: vec![EnumDef {
            name: "Message".into(),
            rust_path: "fixture::Message".into(),
            serde_tag: tag.map(str::to_owned),
            serde_content: content.map(str::to_owned),
            serde_untagged: untagged,
            variants: vec![EnumVariant {
                name: "User".into(),
                serde_rename: Some("user".into()),
                is_tuple: tuple,
                fields: vec![FieldDef {
                    name: "_0".into(),
                    ty: TypeRef::Named("UserMessage".into()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
        types: vec![TypeDef {
            name: "UserMessage".into(),
            rust_path: "fixture::UserMessage".into(),
            fields: vec![FieldDef {
                name: "content".into(),
                ty: TypeRef::String,
                ..Default::default()
            }],
            has_serde: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    (api, config.resolve().unwrap().remove(0))
}

fn generated_message(tag: Option<&str>, content: Option<&str>, untagged: bool, tuple: bool) -> syn::ItemEnum {
    generated_files(tag, content, untagged, tuple)
        .iter()
        .filter(|file| file.path.ends_with("lib.rs"))
        .flat_map(|file| syn::parse_file(&file.content).unwrap().items)
        .find_map(|item| match item {
            syn::Item::Enum(item) if item.ident == "Message" => Some(item),
            _ => None,
        })
        .expect("actual backend must emit Message")
}

/// `native.rb` is emitted by `generate_public_api`, not `generate_bindings` -- the latter only
/// produces the Rust extension crate. Reading it from the wrong entry point yields an empty file
/// set, which looks identical to "the backend stopped emitting it". ~keep
fn generated_native_rb(tag: Option<&str>, content: Option<&str>, untagged: bool, tuple: bool) -> String {
    let (api, crate_config) = fixture_api(tag, content, untagged, tuple);
    MagnusBackend
        .generate_public_api(&api, &crate_config)
        .unwrap()
        .into_iter()
        .find(|file| file.path.ends_with("native.rb"))
        .expect("actual backend must emit native.rb")
        .content
}

fn has_flatten(field: &syn::Field) -> bool {
    field.attrs.iter().any(|attr| {
        attr.path().is_ident("serde") && attr.parse_args::<syn::Ident>().is_ok_and(|ident| ident == "flatten")
    })
}

#[test]
fn internally_tagged_newtype_preserves_flat_payload_in_actual_backend() {
    let item = generated_message(Some("role"), None, false, true);
    let field = item.variants[0].fields.iter().next().unwrap();
    assert!(matches!(item.variants[0].fields, syn::Fields::Named(_)));
    assert!(has_flatten(field), "synthetic _0 must not become a required wire key");
}

/// The Ruby `Data.define` wrapper's `from_hash` is the consumer-facing half of the same fix the
/// two assertions above pin on the binding: once the binding flattens, the wire carries the
/// payload's own keys and no `_0`, so `hash[:_0]` resolved to `nil` for every such variant and
/// `value` was silently always nil.
#[test]
fn should_not_emit_underscore_zero_key_for_flattened_newtype_variant() {
    let native_rb = generated_native_rb(Some("role"), None, false, true);

    assert!(
        !native_rb.contains("hash[:_0]"),
        "the flattened payload has no `_0` key to read, got:\n{native_rb}"
    );
    assert!(
        native_rb.contains(r#"payload = hash.reject { |key, _| key.to_s == "role" }.transform_keys(&:to_sym)"#),
        "the payload must be built from the tag-stripped hash, got:\n{native_rb}"
    );
    assert!(
        native_rb.contains("new(value: UserMessage.new(payload))"),
        "the payload must be constructed as its declared type, got:\n{native_rb}"
    );
}

/// The `_0` hop is CORRECT for every other representation — external tagging keys the payload on
/// the variant name, adjacent tagging on the content key, untagged writes it bare — so the fix
/// must not reach them. Only the internally tagged newtype loses the key.
#[test]
fn should_keep_reading_the_positional_key_for_representations_serde_does_not_flatten() {
    // Each case names the key serde actually writes the payload under, which is the point of
    // the doc comment above: adjacent tagging uses the CONTENT key, and only the representations
    // that write no key of their own fall back to the synthesized positional name. Asserting
    // `_0` for the adjacent case would re-assert the very defect the content-key branch fixes
    // (xberg's `DiffLine`, `tag = "kind", content = "text"`). ~keep
    for (tag, content, untagged, tuple, expected_key) in [
        (Some("role"), Some("payload"), false, true, "payload"),
        (Some("role"), None, false, false, "_0"),
    ] {
        let native_rb = generated_native_rb(tag, content, untagged, tuple);
        let expected = format!(r#"new(value: hash[:{expected_key}] || hash["{expected_key}"])"#);
        assert!(
            native_rb.contains(&expected),
            "tag={tag:?} content={content:?} untagged={untagged} tuple={tuple} must read \
             `{expected_key}`, got:\n{native_rb}"
        );
    }
}

#[test]
fn other_enum_representations_do_not_flatten_payload() {
    for (tag, content, untagged, tuple) in [
        (Some("role"), Some("payload"), false, true),
        (None, None, true, true),
        (None, None, false, true),
        (Some("role"), None, false, false),
    ] {
        let item = generated_message(tag, content, untagged, tuple);
        assert_eq!(item.variants.len(), 1);
        assert!(item.variants[0].fields.iter().all(|field| !has_flatten(field)));
    }
}
