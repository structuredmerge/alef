use super::gen_tagged_enum_ruby_classes;
use crate::core::ir::{EnumDef, EnumVariant, FieldDef, PrimitiveType, TypeRef};
use std::process::Command;

#[test]
fn generated_data_payload_readers_execute_in_ruby() {
    let generated = gen_tagged_enum_ruby_classes(&payload_enum(), "Fixture", &[]);
    let assertions = r#"
payload = "hello"
text = Fixture::MessageText.new(value: payload)
raise "newtype reader lost payload identity" unless text.value.equal?(payload)
raise "newtype predicate changed" unless text.text? && !text.record?
record = Fixture::MessageRecord.new(label: payload, enabled: false, note: nil)
raise "named reader lost payload identity" unless record.label.equal?(payload)
raise "false payload changed" unless record.enabled == false
raise "nil payload changed" unless record.note.nil?
raise "record predicate changed" unless record.record? && !record.text?
Fixture::MessageRecord.class_eval { def to_h = raise "overridden to_h called" }
raise "reader dispatched to overridden to_h" unless record.label.equal?(payload)
empty = Fixture::MessageEmpty.new
raise "unit variant changed" unless empty.empty?
puts "8 runtime checks passed"
"#;
    let script = format!("{generated}\n{assertions}");
    let result = Command::new("ruby")
        .args(["-rsorbet-runtime", "-e", &script])
        .output()
        .expect("Ruby 3.2+ with sorbet-runtime is required for the generated Data regression");
    assert!(
        result.status.success(),
        "generated Ruby failed:\n{}\n{generated}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout).trim(),
        "8 runtime checks passed"
    );
}

fn payload_enum() -> EnumDef {
    EnumDef {
        name: "Message".into(),
        serde_tag: Some("kind".into()),
        serde_content: Some("payload".into()),
        variants: vec![
            EnumVariant {
                name: "Text".into(),
                is_tuple: true,
                fields: vec![field("_0", TypeRef::String, false)],
                ..Default::default()
            },
            EnumVariant {
                name: "Record".into(),
                fields: vec![
                    field("label", TypeRef::String, false),
                    field("enabled", TypeRef::Primitive(PrimitiveType::Bool), false),
                    field("note", TypeRef::String, true),
                ],
                ..Default::default()
            },
            EnumVariant {
                name: "Empty".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

fn field(name: &str, ty: TypeRef, optional: bool) -> FieldDef {
    FieldDef {
        name: name.into(),
        ty,
        optional,
        ..Default::default()
    }
}
