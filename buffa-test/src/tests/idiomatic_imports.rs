//! Runtime tests for code generated with `idiomatic_imports(true)`.
//!
//! Compilation of `crate::idiomatic` already proves the `use`-backed short
//! names resolve (bare leaf, extern import, parent-module qualification,
//! reserved-name fallback, nested-message depth). These tests verify the
//! generated code is semantically equivalent to default codegen: wire
//! round-trips, correct variant typing, and view decoding all behave
//! identically.

use super::round_trip;
use crate::idiomatic::__buffa::oneof::holder::Kind;
use crate::idiomatic::__buffa::oneof::{outer, shadow_holder};
use crate::idiomatic::{outer_a, outer_b, Holder, ShadowHolder, Target};
use buffa::{Message, MessageView};
use buffa_types::google::protobuf::Timestamp;

#[test]
fn same_package_variant_round_trips() {
    let msg = Holder {
        kind: Some(Kind::from(Target {
            name: "hello".into(),
            ..Default::default()
        })),
        ..Default::default()
    };
    let back = round_trip(&msg);
    assert_eq!(back, msg);
    match back.kind {
        Some(Kind::Target(t)) => assert_eq!(t.name, "hello"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn extern_wkt_variant_round_trips() {
    // `Kind::At(Box<Timestamp>)` — Timestamp is the buffa-types WKT,
    // referenced through a `use` directive in the generated module.
    let msg = Holder {
        kind: Some(Kind::At(Box::new(Timestamp {
            seconds: 1_000_000_000,
            nanos: 42,
            ..Default::default()
        }))),
        ..Default::default()
    };
    assert_eq!(round_trip(&msg), msg);
}

#[test]
fn colliding_inner_variants_are_distinct_types() {
    // OuterA.Inner (bare `Inner`) and OuterB.Inner (`outer_b::Inner`) are
    // different Rust types despite the shared leaf name.
    let a = Holder {
        kind: Some(Kind::from(outer_a::Inner {
            a: 1,
            ..Default::default()
        })),
        ..Default::default()
    };
    let b = Holder {
        kind: Some(Kind::from(outer_b::Inner {
            b: 2,
            ..Default::default()
        })),
        ..Default::default()
    };
    assert_eq!(round_trip(&a), a);
    assert_eq!(round_trip(&b), b);
    assert_ne!(a, b);
}

#[test]
fn reserved_enum_name_variant_round_trips() {
    // ShadowHolder's oneof is named `target`, so its enum ident `Target`
    // occupies the module scope and the message-`Target` variant keeps a
    // fully-qualified type. Same wire behaviour either way.
    let msg = ShadowHolder {
        target: Some(shadow_holder::Target::from(Target {
            name: "shadowed".into(),
            ..Default::default()
        })),
        ..Default::default()
    };
    let back = round_trip(&msg);
    match back.target {
        Some(shadow_holder::Target::Shadowed(t)) => assert_eq!(t.name, "shadowed"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn nested_message_oneof_resolves_at_depth_four() {
    // Outer.Mid's oneof enum lives at `__buffa::oneof::outer::mid`; its
    // `use` directives carry an extra `super::` hop. Compilation proves
    // resolution; this exercises the values.
    use crate::idiomatic::outer::Mid;
    use outer::mid::Kind as MidKind;

    let same_pkg = Mid {
        kind: Some(MidKind::from(Target {
            name: "deep".into(),
            ..Default::default()
        })),
        ..Default::default()
    };
    assert_eq!(round_trip(&same_pkg), same_pkg);

    let extern_wkt = Mid {
        kind: Some(MidKind::DeepAt(Box::new(Timestamp {
            seconds: 1,
            ..Default::default()
        }))),
        ..Default::default()
    };
    assert_eq!(round_trip(&extern_wkt), extern_wkt);

    let sibling_nested = Mid {
        kind: Some(MidKind::from(outer_a::Inner {
            a: 3,
            ..Default::default()
        })),
        ..Default::default()
    };
    assert_eq!(round_trip(&sibling_nested), sibling_nested);
}

#[test]
fn view_decode_matches_owned() {
    let msg = Holder {
        kind: Some(Kind::Num(7)),
        ..Default::default()
    };
    let bytes = msg.encode_to_vec();
    let view = crate::idiomatic::HolderView::decode_view(&bytes).expect("view decode");
    assert_eq!(view.to_owned_message(), msg);
}
