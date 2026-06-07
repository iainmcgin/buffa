# Idiomatic imports in generated code — design note

Spike for an emit-time type registry (`ScopeImports`) that lets generated
code reference types through `use` directives and short names — reading like
hand-written Rust — instead of `super::`-chained or absolute paths. Gated
behind the off-by-default `idiomatic_imports` option (`CodeGenConfig` field,
`buffa_build::Config::idiomatic_imports`, plugin parameter
`idiomatic_imports=true`); default output is byte-for-byte unchanged
(verified by regenerating `buffa-types/src/generated/` and
`buffa-descriptor/src/generated/` with the flag off — zero diff — plus an
explicit byte-equality test).

## Before / after

A oneof enum at `__buffa::oneof::holder` (three module levels below the
package root), default output:

```rust,ignore
pub enum Kind {
    Target(::buffa::alloc::boxed::Box<super::super::super::Target>),
    At(::buffa::alloc::boxed::Box<::buffa_types::google::protobuf::Timestamp>),
    InnerA(::buffa::alloc::boxed::Box<super::super::super::outer_a::Inner>),
    InnerB(::buffa::alloc::boxed::Box<super::super::super::outer_b::Inner>),
}
```

With `idiomatic_imports=true`:

```rust,ignore
use super::super::super::outer_a::Inner;
use super::super::super::Target;
use ::buffa_types::google::protobuf::Timestamp;
use super::super::super::outer_b;

pub enum Kind {
    Target(::buffa::alloc::boxed::Box<Target>),
    At(::buffa::alloc::boxed::Box<Timestamp>),
    InnerA(::buffa::alloc::boxed::Box<Inner>),
    InnerB(::buffa::alloc::boxed::Box<outer_b::Inner>),
}
```

(`Box` stays absolute per the `no_std` rule — see "what stays qualified".)

## The stitcher constraint, validated

The stitcher merges **all content files of a package into shared module
scopes via `include!`**: every `.proto` in a package contributes to the
package root, and every `.__oneof.rs` content file is included into the one
`pub mod __buffa { pub mod oneof { … } }` block authored by the package's
`.mod.rs` (`generate_package_mod` inlines the same shapes in
`file_per_package` mode). buffa-types is the canonical consumer-side
example: seven generated files share one hand-written `pub mod protobuf`.
Per-file codegen therefore can never enumerate the names in scope at the
package root or inside the shared `__buffa::oneof` wrapper, and a `use`
emitted there could collide with a sibling file's items. The existing
`ALLOW_LINTS` entry for `unused_qualifications` documents the same point
from the other direction: canonical qualified paths are used *because* bare
names are not stable under merging.

What does survive merging intact is the **per-message module**: protoc
guarantees message names are unique within a package, so
`__buffa::oneof::<msg_path>` (and its nested `pub mod` chain for nested
messages) has fully known contents at generation time. That is where the
registry emits its `use` directives.

**Scope consequence:** the only message/enum type references that live
inside per-message modules are **oneof variant types** (owned and view).
Owned struct fields, `impl Message` bodies, and view structs are emitted at
package scope (or the shared `view` wrapper) and cannot be import-shortened
under this design. The headline verbosity on struct fields
(`::buffa::MessageField<super::super::context::v1::RequestContext>`)
therefore stays. The spike covers the owned oneof enums; view-oneof enums
are the natural next increment.

## The registry

`ScopeImports` (`buffa-codegen/src/imports.rs`) is created per message in
`generate_message_with_nesting` for that message's `__buffa::oneof::…`
module, threaded into `generate_oneof_enum` → `collect_variant_info`, and
its recorded `use` block is emitted into the module wrap. `resolve(path)`
applies a deterministic alias ladder:

1. **Bare leaf name** backed by `use <path>;` —
   `use super::super::super::Target;` … `Target`,
   `use ::buffa_types::google::protobuf::Timestamp;` … `Timestamp`.
2. **Parent-module qualification** on collision —
   `use super::super::super::outer_b;` … `outer_b::Inner`.
3. **Fully qualified** otherwise. `use … as Alias` renames were considered
   and rejected: `outer_b::Inner` reads better than synthetic
   `OuterBInner`-style aliases, and rung 3 almost never fires in practice.

Soundness rules:

- **Explicit `use` only — never glob reliance.** An earlier draft (against
  the pre-`__buffa` layout) let same-package names resolve bare through the
  `use super::*;` glob chain. In the current layout that is unsound: the
  chain passes through the shared `oneof` wrapper, whose items (sibling
  message modules like `pub mod target`) and ancestor oneof modules' enums
  can shadow a glob-resolved name — silently, since a oneof named `inner`
  produces an enum `Inner`. Explicit imports shadow glob imports
  deterministically (and are immune to rustc's glob-vs-`include!` E0659
  ambiguity), so every short name is backed by a `use`.
- **Reserved names.** A short name is never claimed if it collides with an
  item defined in the same module — the message's oneof enum idents and
  nested-message sub-module names (`oneof_module_scope_names`) — or with a
  fixed list of prelude/primitive names that generated code in these
  modules references bare (`From` in the From-impls, `derive(Clone, …)`,
  scalar variant types like `bool`).
- **One name, one path.** Claims live in a `BTreeMap` (deterministic,
  alphabetically ordered `use` block); a second path with the same leaf
  falls down the ladder. Identical proto types resolve to identical tokens,
  so the `From`-impl dedup keyed on `TokenStream::to_string()` still holds.
  Note the ordering is by *short name*, not rustfmt's path-grouped order —
  `::`-rooted and `super::`-rooted imports interleave. The use-site win is
  the real payoff; the import block itself still carries the `super::`
  chains, just hoisted. Rustfmt-style grouping is a possible polish item.
- **Imports are recorded only at use sites**, so every emitted `use` is
  referenced — `unused_imports` cannot fire and no new `#[allow]` was
  needed (in the codegen crate or the generated output).
- **What stays qualified:** prelude/alloc/runtime types
  (`::buffa::alloc::boxed::Box`, `::core::option::Option`, `::buffa::…`)
  never go through the registry, per the `no_std` and include!-safety rules
  at the top of `imports.rs`. Token emission stays on
  `rust_path_to_tokens` (keyword segments like `type` → `r#type`);
  `syn::parse_str` is still avoided.

One subtlety: the oneof codegen gated `From<T> for Option<Oneof>` on the
*emitted tokens* starting with `::` (orphan-rule check for extern types). A
shortened `Timestamp` no longer reveals extern-ness, so `is_extern` is now
recorded from the resolved path string before shortening
(`ScopedType` from `scalar_or_message_type_scoped` in `message.rs`) and
carried in `VariantInfo`. Never re-derive extern-ness from tokens.

## Answers to the brief's open questions

1. *Does emitting `use` inside per-message scopes fully sidestep the
   `include!` problem?* For those scopes, yes — but package-root references
   (struct fields, impls, registry consts, natural-path re-exports,
   extension paths) and the shared `__buffa`/`oneof`/`view` wrappers are
   merged scopes and must stay qualified. The `unused_qualifications` allow
   in `ALLOW_LINTS` remains necessary for those emissions.
2. *Is `resolve_type_path`'s `(rust_path, is_extern)` sufficient to drive
   the registry?* Nearly. The flat path string from `rust_type_relative`
   (nesting hops baked in) plus extern-ness is exactly the registry input;
   no additional nesting context was needed at call sites because
   `collect_variant_info` already receives the enum body's total depth
   (`nesting + 3`). What was needed beyond the pair: (a) the reserved-name
   set, computed per message from the descriptor, and (b) carrying
   `is_extern` out-of-band for the orphan-rule gate.
   `rust_type_relative_split` / `SplitPath` would allow a more structured
   handoff (`to_package` + `within_package`), but the flat string was
   sufficient for this category.
3. *Is importing the `extern_path` target directly always safe?* Yes, with
   the same caveats as local names: the import is safe because the registry
   owns all short names in the scope and refuses collisions. The path
   string (absolute `::crate::…` or `crate::…`) is used verbatim as the
   `use` target, so no assumptions about the sibling crate's layout are
   added beyond what inline qualified paths already assumed.

## Spike scope and possible next steps

Covered end to end: owned oneof variant types (message and enum), all three
ladder rungs, reserved-name refusal, extern (`extern_path`/WKT) imports,
nested-message depth, with snapshot tests
(`buffa-codegen/src/tests/idiomatic_imports.rs`, `imports.rs` unit tests)
and a compiled + runtime-verified sample
(`buffa-test/protos/idiomatic_imports.proto`, flag on).

Not attempted, in rough order of value if the direction is kept:

- **View-oneof enums** (`__buffa::view::oneof::<msg>`) — same modules, same
  mechanism; needs view-type names (`TimestampView`) in the registry.
- **Custom-deserialize oneof arms** (`custom_deser_oneof_group`) re-resolve
  variant types at struct scope (package level) and are untouched; with
  `json=true` the enum declaration and the deserialize impl render the same
  type differently. Cosmetic, but worth unifying if the flag graduates.
- **Parameter-bundle refactor:** the spike threads `&mut ScopeImports`
  through `generate_oneof_enum` → `collect_variant_info`, each already
  carrying `#[allow(clippy::too_many_arguments)]`. A context-struct bundle
  (or folding per-scope state into `ImportResolver`, whose header comment
  anticipates exactly this) should land before more per-scope state does.
- **The package-scope question** — whether any acceptable mechanism exists
  for struct-field types — is the real go/no-go for "generated code reads
  like hand-written Rust" and deserves its own decision. Candidates
  (per-file wrapper modules; `pub use` blocks) all have costs the brief's
  constraint was designed to avoid.
