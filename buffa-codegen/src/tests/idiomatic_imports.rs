//! End-to-end tests for `CodeGenConfig::idiomatic_imports`.
//!
//! With the flag off, output must be byte-for-byte identical to the default;
//! with the flag on, message/enum types referenced by oneof variants are
//! shortened to `use`-backed names inside the message's
//! `__buffa::oneof::<msg>` module, falling back to parent-module
//! qualification and fully-qualified paths on collisions.

use super::*;

fn idiomatic_config() -> CodeGenConfig {
    CodeGenConfig {
        idiomatic_imports: true,
        ..Default::default()
    }
}

fn message_field(
    name: &str,
    number: i32,
    type_name: &str,
    oneof_index: i32,
) -> FieldDescriptorProto {
    FieldDescriptorProto {
        name: Some(name.to_string()),
        number: Some(number),
        label: Some(Label::LABEL_OPTIONAL),
        r#type: Some(Type::TYPE_MESSAGE),
        type_name: Some(type_name.to_string()),
        oneof_index: Some(oneof_index),
        ..Default::default()
    }
}

fn kind_oneof() -> OneofDescriptorProto {
    OneofDescriptorProto {
        name: Some("kind".to_string()),
        ..Default::default()
    }
}

/// A package with a plain `Target` message and a `Holder` whose oneof
/// references it.
fn same_package_file() -> FileDescriptorProto {
    let mut file = proto3_file("holder.proto");
    file.package = Some("test.pkg".to_string());
    file.message_type.push(DescriptorProto {
        name: Some("Target".to_string()),
        ..Default::default()
    });
    file.message_type.push(DescriptorProto {
        name: Some("Holder".to_string()),
        field: vec![message_field("t", 1, ".test.pkg.Target", 0)],
        oneof_decl: vec![kind_oneof()],
        ..Default::default()
    });
    file
}

#[test]
fn flag_off_keeps_qualified_paths() {
    let files = generate(
        &[same_package_file()],
        &["holder.proto".to_string()],
        &CodeGenConfig::default(),
    )
    .expect("should generate");
    let content = joined(&files);
    // Oneof enum body sits at `__buffa::oneof::holder` — three module
    // levels below the package root.
    assert!(
        content.contains("T(::buffa::alloc::boxed::Box<super::super::super::Target>)"),
        "default output must keep the qualified variant type: {content}"
    );
}

#[test]
fn same_package_variant_becomes_bare_name() {
    let files = generate(
        &[same_package_file()],
        &["holder.proto".to_string()],
        &idiomatic_config(),
    )
    .expect("should generate");
    let content = joined(&files);
    assert!(
        content.contains("use super::super::super::Target;"),
        "same-package reference should produce a use directive: {content}"
    );
    assert!(
        content.contains("T(::buffa::alloc::boxed::Box<Target>)"),
        "variant should use the bare name: {content}"
    );
    // From impls use the same shortened tokens, and `From<T> for
    // Option<Oneof>` survives for local types.
    assert!(
        content.contains("impl From<Target> for Kind"),
        "From impl should use the bare name: {content}"
    );
    assert!(
        content.contains("impl From<Target> for ::core::option::Option<Kind>"),
        "local types keep the Option From impl: {content}"
    );
}

#[test]
fn extern_wkt_variant_gets_use_directive() {
    let mut file = proto3_file("holder.proto");
    file.package = Some("test.pkg".to_string());
    file.dependency = vec!["google/protobuf/timestamp.proto".to_string()];
    file.message_type.push(DescriptorProto {
        name: Some("Holder".to_string()),
        field: vec![message_field("at", 1, ".google.protobuf.Timestamp", 0)],
        oneof_decl: vec![kind_oneof()],
        ..Default::default()
    });

    let mut ts_file = proto3_file("google/protobuf/timestamp.proto");
    ts_file.package = Some("google.protobuf".to_string());
    ts_file.message_type.push(DescriptorProto {
        name: Some("Timestamp".to_string()),
        ..Default::default()
    });

    let files = generate(
        &[file, ts_file],
        &["holder.proto".to_string()],
        &idiomatic_config(),
    )
    .expect("should generate");
    let content = joined(&files);
    assert!(
        content.contains("use ::buffa_types::google::protobuf::Timestamp;"),
        "extern reference should produce a use directive: {content}"
    );
    assert!(
        content.contains("At(::buffa::alloc::boxed::Box<Timestamp>)"),
        "variant should use the imported name: {content}"
    );
    // Extern gating must survive the shortening: `From<T> for Oneof` is
    // legal, but `From<T> for Option<Oneof>` would violate the orphan rule.
    assert!(
        content.contains("impl From<Timestamp> for Kind"),
        "From<T> for Oneof should still be emitted: {content}"
    );
    assert!(
        !content.contains("impl From<Timestamp> for ::core::option::Option<Kind>"),
        "From<T> for Option<Oneof> must stay suppressed for extern types: {content}"
    );
}

#[test]
fn oneof_enum_name_collision_stays_qualified() {
    // The oneof is named `target`, so its enum ident `Target` occupies the
    // module scope — the variant referencing message `Target` must stay
    // qualified (a `use` would be E0255 against the enum).
    let mut file = proto3_file("holder.proto");
    file.package = Some("test.pkg".to_string());
    file.message_type.push(DescriptorProto {
        name: Some("Target".to_string()),
        ..Default::default()
    });
    file.message_type.push(DescriptorProto {
        name: Some("Holder".to_string()),
        field: vec![message_field("t", 1, ".test.pkg.Target", 0)],
        oneof_decl: vec![OneofDescriptorProto {
            name: Some("target".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    });

    let files = generate(&[file], &["holder.proto".to_string()], &idiomatic_config())
        .expect("should generate");
    let content = joined(&files);
    assert!(
        content.contains("T(::buffa::alloc::boxed::Box<super::super::super::Target>)"),
        "shadowed name must stay qualified: {content}"
    );
    assert!(
        !content.contains("use super::super::super::Target;"),
        "no use directive may collide with the oneof enum: {content}"
    );
}

#[test]
fn duplicate_leaf_uses_parent_module_qualification() {
    // Two nested `Inner` types under different outers, both referenced from
    // one oneof: the first claims the bare name, the second is qualified
    // through its imported parent module.
    let mut file = proto3_file("holder.proto");
    file.package = Some("test.pkg".to_string());
    for outer in ["OuterA", "OuterB"] {
        file.message_type.push(DescriptorProto {
            name: Some(outer.to_string()),
            nested_type: vec![DescriptorProto {
                name: Some("Inner".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        });
    }
    file.message_type.push(DescriptorProto {
        name: Some("Holder".to_string()),
        field: vec![
            message_field("a", 1, ".test.pkg.OuterA.Inner", 0),
            message_field("b", 2, ".test.pkg.OuterB.Inner", 0),
        ],
        oneof_decl: vec![kind_oneof()],
        ..Default::default()
    });

    let files = generate(&[file], &["holder.proto".to_string()], &idiomatic_config())
        .expect("should generate");
    let content = joined(&files);
    assert!(
        content.contains("use super::super::super::outer_a::Inner;"),
        "first Inner should be imported: {content}"
    );
    assert!(
        content.contains("A(::buffa::alloc::boxed::Box<Inner>)"),
        "first Inner should use the bare name: {content}"
    );
    assert!(
        content.contains("use super::super::super::outer_b;"),
        "second Inner's parent module should be imported: {content}"
    );
    assert!(
        content.contains("B(::buffa::alloc::boxed::Box<outer_b::Inner>)"),
        "second Inner should be parent-qualified: {content}"
    );
}

#[test]
fn enum_variant_shortens_inside_enum_value() {
    let mut file = proto3_file("holder.proto");
    file.package = Some("test.pkg".to_string());
    file.enum_type.push(EnumDescriptorProto {
        name: Some("Status".to_string()),
        value: vec![enum_value("UNKNOWN", 0)],
        ..Default::default()
    });
    file.message_type.push(DescriptorProto {
        name: Some("Holder".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("status".to_string()),
            number: Some(1),
            label: Some(Label::LABEL_OPTIONAL),
            r#type: Some(Type::TYPE_ENUM),
            type_name: Some(".test.pkg.Status".to_string()),
            oneof_index: Some(0),
            ..Default::default()
        }],
        oneof_decl: vec![kind_oneof()],
        ..Default::default()
    });

    let files = generate(&[file], &["holder.proto".to_string()], &idiomatic_config())
        .expect("should generate");
    let content = joined(&files);
    assert!(
        content.contains("Status(::buffa::EnumValue<Status>)"),
        "open-enum variant should shorten the inner type: {content}"
    );
    assert!(
        content.contains("use super::super::super::Status;"),
        "enum reference should produce a use directive: {content}"
    );
}

#[test]
fn nested_message_oneof_imports_at_its_own_depth() {
    // A oneof inside a nested message: its enum lives at
    // `__buffa::oneof::outer::mid` (depth 4), so the recorded `use`
    // directives carry one more `super::` hop.
    let mut file = proto3_file("holder.proto");
    file.package = Some("test.pkg".to_string());
    file.message_type.push(DescriptorProto {
        name: Some("Target".to_string()),
        ..Default::default()
    });
    file.message_type.push(DescriptorProto {
        name: Some("Outer".to_string()),
        nested_type: vec![DescriptorProto {
            name: Some("Mid".to_string()),
            field: vec![message_field("deep", 1, ".test.pkg.Target", 0)],
            oneof_decl: vec![kind_oneof()],
            ..Default::default()
        }],
        ..Default::default()
    });

    let files = generate(&[file], &["holder.proto".to_string()], &idiomatic_config())
        .expect("should generate");
    let content = joined(&files);
    assert!(
        content.contains("use super::super::super::super::Target;"),
        "depth-4 scope should import with four super hops: {content}"
    );
    assert!(
        content.contains("Deep(::buffa::alloc::boxed::Box<Target>)"),
        "depth-4 variant should use the bare name: {content}"
    );
}

#[test]
fn serde_module_name_never_imported_under_json() {
    // With `generate_json` on, the oneof module also contains `Serialize`
    // impls that reference the `serde` crate by bare path. A message named
    // `Serde` snake-cases to module `serde`; a rung-2 parent-module import
    // of it would shadow the crate. The reference must stay fully qualified
    // while the bare-path serde references keep working.
    let mut file = proto3_file("holder.proto");
    file.package = Some("test.pkg".to_string());
    for outer in ["Wrap", "Serde"] {
        file.message_type.push(DescriptorProto {
            name: Some(outer.to_string()),
            nested_type: vec![DescriptorProto {
                name: Some("Inner".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        });
    }
    file.message_type.push(DescriptorProto {
        name: Some("Holder".to_string()),
        field: vec![
            message_field("a", 1, ".test.pkg.Wrap.Inner", 0),
            message_field("b", 2, ".test.pkg.Serde.Inner", 0),
        ],
        oneof_decl: vec![kind_oneof()],
        ..Default::default()
    });

    let config = CodeGenConfig {
        idiomatic_imports: true,
        generate_json: true,
        ..Default::default()
    };
    let files = generate(&[file], &["holder.proto".to_string()], &config).expect("should generate");
    let content = joined(&files);
    assert!(
        !content.contains("use super::super::super::serde;"),
        "the serde crate name must never be claimed by an import: {content}"
    );
    assert!(
        content.contains("B(::buffa::alloc::boxed::Box<super::super::super::serde::Inner>)"),
        "the colliding Inner should stay fully qualified: {content}"
    );
    assert!(
        content.contains("impl serde::Serialize for Kind"),
        "bare-path serde references must remain intact: {content}"
    );
}

#[test]
fn flag_off_output_is_identical_to_default() {
    // Belt-and-braces: an explicit `idiomatic_imports: false` config and the
    // default config must produce identical bytes for a oneof-heavy file.
    let default_out = generate(
        &[same_package_file()],
        &["holder.proto".to_string()],
        &CodeGenConfig::default(),
    )
    .expect("should generate");
    let off_explicit = generate(
        &[same_package_file()],
        &["holder.proto".to_string()],
        &CodeGenConfig {
            idiomatic_imports: false,
            ..Default::default()
        },
    )
    .expect("should generate");
    assert_eq!(default_out.len(), off_explicit.len());
    for (a, b) in default_out.iter().zip(&off_explicit) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.content, b.content);
    }
}
