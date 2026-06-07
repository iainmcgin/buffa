//! Import management for generated code.
//!
//! Prelude types like `Option` are always emitted as fully-qualified paths
//! (`::core::option::Option<T>`) to prevent shadowing by proto-defined types
//! of the same name. This is necessary because the stitcher combines all
//! files from one package into a single module scope via `include!`, so a
//! `message Option` in *any* sibling file would shadow the prelude.
//!
//! `alloc` types (`String`, `Vec`, `Box`) are always emitted as
//! `::buffa::alloc::*` paths because they are not in the `no_std` prelude,
//! consistent with the `HashMap` approach via `::buffa::__private::HashMap`.
//! Buffa runtime types are always emitted as absolute paths since generated
//! files may be combined via `include!`.
//!
//! [`ScopeImports`] (gated behind `CodeGenConfig::idiomatic_imports`) layers
//! per-module-scope `use` directives on top: inside scopes that are unique
//! per message — and therefore survive `include!` merging without
//! cross-file collisions — qualified type paths can be shortened to
//! `use`-backed short names.

use std::collections::{BTreeMap, HashSet};

use crate::idents::rust_path_to_tokens;
use proc_macro2::TokenStream;
use quote::quote;

/// Single source of truth for type-path emission in generated code.
///
/// All prelude types are unconditionally emitted as fully-qualified paths
/// (e.g. `::core::option::Option`) to avoid shadowing by user-defined proto
/// types. This is simpler and more robust than trying to detect collisions:
/// the stitcher's `include!`-based module merging makes it impossible to
/// know at per-file generation time which names will be in scope.
///
/// Stateless — kept as a struct (rather than free functions) so call sites
/// uniformly take `&ImportResolver` and any future per-scope state can be
/// added without re-threading parameters.
pub(crate) struct ImportResolver;

impl ImportResolver {
    pub fn new() -> Self {
        Self
    }

    // ── Prelude type tokens ─────────────────────────────────────────────

    pub fn option(&self) -> TokenStream {
        quote! { ::core::option::Option }
    }

    // ── Alloc types (always absolute, no_std-safe via ::buffa::alloc) ───

    pub fn string(&self) -> TokenStream {
        quote! { ::buffa::alloc::string::String }
    }

    pub fn vec(&self) -> TokenStream {
        quote! { ::buffa::alloc::vec::Vec }
    }

    // ── Buffa runtime types (always absolute, include!-safe) ────────────

    pub fn message_field(&self) -> TokenStream {
        quote! { ::buffa::MessageField }
    }

    pub fn enum_value(&self) -> TokenStream {
        quote! { ::buffa::EnumValue }
    }

    pub fn hashmap(&self) -> TokenStream {
        quote! { ::buffa::__private::HashMap }
    }
}

/// Names the registry must never claim as a short name, regardless of what
/// the scope defines.
///
/// The invariant: **every identifier that any sibling emission in the same
/// oneof module references bare** — prelude traits (`impl From<…>`, the
/// derive paths in `#[derive(Clone, …)]`), primitive scalar variant types
/// (`bool`, `i32`, …), and crate names. `serde` is the one crate referenced
/// by bare path (`impl serde::Serialize`, `use serde::ser::SerializeMap;`
/// in `generate_oneof_serialize` when `generate_json` is on); everything
/// else (`::buffa`, `::core`, `::arbitrary`) is absolute. A `use` claiming
/// any of these would silently change what those emissions mean. When a new
/// emission inside `__buffa::oneof::<msg>` modules references a name bare,
/// it must be added here.
const SCOPE_RESERVED_NAMES: &[&str] = &[
    "Box",
    "Clone",
    "Copy",
    "Debug",
    "Default",
    "Eq",
    "From",
    "Hash",
    "Into",
    "None",
    "Option",
    "PartialEq",
    "Result",
    "Self",
    "Send",
    "Some",
    "String",
    "Sync",
    "Vec",
    "bool",
    "f32",
    "f64",
    "i32",
    "i64",
    "serde",
    "str",
    "u8",
    "u32",
    "u64",
];

/// A short name claimed within a [`ScopeImports`] scope.
struct Binding {
    /// The full path this short name was imported from.
    path: String,
}

/// Per-module-scope import registry for `CodeGenConfig::idiomatic_imports`.
///
/// One instance lives for the duration of a single per-message module inside
/// an ancillary tree (e.g. `__buffa::oneof::<msg_path>`). Message names are
/// unique within a package, so these modules — unlike the package root or
/// the shared `__buffa::oneof` wrapper, which merge content from every
/// `.proto` in the package — have fully known contents at generation time.
///
/// [`resolve`](Self::resolve) takes the relative/absolute Rust path of a
/// referenced type and returns the tokens to emit at the use site, shortening
/// the path when the short form is provably unambiguous and recording the
/// `use` directive that backs it. [`use_items`](Self::use_items) then emits
/// exactly the recorded imports — every one referenced, so `unused_imports`
/// cannot fire.
///
/// Anti-shadowing model:
///
/// - Short names are always backed by an **explicit `use`**, never by the
///   `use super::*;` glob chain. Explicit imports shadow glob imports
///   deterministically, which makes the binding immune to same-named items
///   in the enclosing `oneof`/`__buffa`/package scopes (sibling message
///   modules, ancestor oneof enums, natural-path re-exports).
/// - A short name is never claimed if it collides with an item defined in
///   this scope (the message's oneof enums and nested-message sub-modules)
///   or with [`SCOPE_RESERVED_NAMES`].
/// - Each short name binds to exactly one full path. A second path with the
///   same leaf falls down the alias ladder: parent-module qualification
///   (`use …::outer_b; … outer_b::Inner`), then the fully-qualified path.
///   `as` renames are intentionally not used — `outer_b::Inner` reads better
///   than `OuterBInner`-style aliases.
///
/// When disabled (the default), [`resolve`](Self::resolve) is exactly
/// [`rust_path_to_tokens`], so default output is byte-for-byte unchanged.
pub(crate) struct ScopeImports {
    enabled: bool,
    /// Names defined in this scope — never claimable.
    reserved: HashSet<String>,
    /// Claimed short name → binding. `BTreeMap` for deterministic
    /// `use`-block ordering.
    bindings: BTreeMap<String, Binding>,
}

impl ScopeImports {
    /// Create a registry for one per-message ancillary-module scope.
    ///
    /// `scope_names` are the item names defined in that module (oneof enum
    /// idents, nested-message sub-module names). The fixed
    /// [`SCOPE_RESERVED_NAMES`] are added automatically.
    pub fn new(enabled: bool, scope_names: &HashSet<String>) -> Self {
        let mut reserved: HashSet<String> = scope_names.clone();
        reserved.extend(SCOPE_RESERVED_NAMES.iter().map(|s| s.to_string()));
        ScopeImports {
            enabled,
            reserved,
            bindings: BTreeMap::new(),
        }
    }

    /// Resolve a type path to the tokens to emit at the use site, recording
    /// an import when the path is shortened.
    ///
    /// `path` is a `::`-separated Rust path as produced by
    /// [`CodeGenContext::rust_type_relative`] — relative
    /// (`super::super::super::Foo`) for local types, absolute
    /// (`::buffa_types::…::Foo`, `crate::gen::Foo`) for extern types.
    ///
    /// [`CodeGenContext::rust_type_relative`]: crate::context::CodeGenContext::rust_type_relative
    pub fn resolve(&mut self, path: &str) -> TokenStream {
        if !self.enabled {
            return rust_path_to_tokens(path);
        }
        // A bare path is already as short as it gets (and claiming it would
        // be self-referential).
        let Some((parent, leaf)) = path.rsplit_once("::") else {
            return rust_path_to_tokens(path);
        };
        if !Self::importable_name(leaf) {
            return rust_path_to_tokens(path);
        }

        // Rung 1: bare leaf name backed by `use <path>;`.
        if self.try_claim(leaf, path) {
            return rust_path_to_tokens(leaf);
        }

        // Rung 2: import the parent module and qualify with one segment.
        if let Some(parent_leaf) = parent.rsplit("::").next() {
            if Self::importable_name(parent_leaf) && self.try_claim(parent_leaf, parent) {
                return rust_path_to_tokens(&format!("{parent_leaf}::{leaf}"));
            }
        }

        // Rung 3: keep the fully-qualified path. (`use … as Alias` renames
        // are deliberately not emitted — see the type-level docs.)
        rust_path_to_tokens(path)
    }

    /// Emit the `use` directives recorded by [`resolve`](Self::resolve), in
    /// deterministic (alphabetical-by-short-name) order.
    pub fn use_items(&self) -> TokenStream {
        let mut out = TokenStream::new();
        for binding in self.bindings.values() {
            let path = rust_path_to_tokens(&binding.path);
            out.extend(quote! { use #path; });
        }
        out
    }

    /// Claim `name` for `path`, or confirm an identical existing claim.
    fn try_claim(&mut self, name: &str, path: &str) -> bool {
        if self.reserved.contains(name) {
            return false;
        }
        match self.bindings.get(name) {
            Some(existing) => existing.path == path,
            None => {
                self.bindings.insert(
                    name.to_string(),
                    Binding {
                        path: path.to_string(),
                    },
                );
                true
            }
        }
    }

    /// Whether `name` can appear as the short side of a `use` binding: not a
    /// path-position keyword (`self`/`super`/`Self`/`crate`, which cannot be
    /// raw idents) and not empty. Assumes `name` is otherwise a valid
    /// identifier — callers pass proto type/package segments, which always
    /// are; raw-able keywords are escaped later by [`rust_path_to_tokens`].
    fn importable_name(name: &str) -> bool {
        !name.is_empty() && !matches!(name, "self" | "super" | "Self" | "crate")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(names: &[&str]) -> ScopeImports {
        let set: HashSet<String> = names.iter().map(|s| s.to_string()).collect();
        ScopeImports::new(true, &set)
    }

    #[test]
    fn disabled_is_passthrough() {
        let mut s = ScopeImports::new(false, &HashSet::new());
        assert_eq!(
            s.resolve("super::super::super::Foo").to_string(),
            rust_path_to_tokens("super::super::super::Foo").to_string()
        );
        assert!(s.use_items().is_empty());
    }

    #[test]
    fn same_package_binds_bare_with_use() {
        let mut s = scope(&[]);
        assert_eq!(s.resolve("super::super::super::Foo").to_string(), "Foo");
        assert_eq!(
            s.use_items().to_string(),
            "use super :: super :: super :: Foo ;"
        );
    }

    #[test]
    fn extern_path_binds_bare_with_use() {
        let mut s = scope(&[]);
        assert_eq!(
            s.resolve("::buffa_types::google::protobuf::Timestamp")
                .to_string(),
            "Timestamp"
        );
        assert_eq!(
            s.use_items().to_string(),
            "use :: buffa_types :: google :: protobuf :: Timestamp ;"
        );
    }

    #[test]
    fn reserved_name_stays_qualified_when_parent_is_super() {
        let mut s = scope(&["Foo"]);
        // Leaf reserved and the parent rung has no importable segment
        // (`super` chains only) → fully qualified.
        assert_eq!(
            s.resolve("super::super::super::Foo").to_string(),
            "super :: super :: super :: Foo"
        );
        assert!(s.use_items().is_empty());
    }

    #[test]
    fn reserved_leaf_falls_to_parent_module() {
        let mut s = scope(&["Inner"]);
        assert_eq!(
            s.resolve("super::super::super::outer::Inner").to_string(),
            "outer :: Inner"
        );
        assert_eq!(
            s.use_items().to_string(),
            "use super :: super :: super :: outer ;"
        );
    }

    #[test]
    fn prelude_names_never_claimed() {
        let mut s = scope(&[]);
        // A proto type named `From` must not shadow the prelude trait that
        // the generated `impl From<…>` blocks in the same scope reference
        // bare; it falls to parent-module qualification instead.
        assert_eq!(
            s.resolve("super::super::super::other::From").to_string(),
            "other :: From"
        );
        assert_eq!(
            s.use_items().to_string(),
            "use super :: super :: super :: other ;"
        );
    }

    #[test]
    fn duplicate_leaf_falls_to_parent_module() {
        let mut s = scope(&[]);
        assert_eq!(
            s.resolve("super::super::super::outer_a::Inner").to_string(),
            "Inner"
        );
        // Second `Inner` from a different parent: qualify via that parent.
        assert_eq!(
            s.resolve("super::super::super::outer_b::Inner").to_string(),
            "outer_b :: Inner"
        );
        assert_eq!(
            s.use_items().to_string(),
            "use super :: super :: super :: outer_a :: Inner ; \
             use super :: super :: super :: outer_b ;"
        );
    }

    #[test]
    fn serde_module_never_claimed() {
        // `generate_oneof_serialize` references the `serde` crate by bare
        // path from the same module; a rung-2 claim of a proto module named
        // `serde` (message `Serde`) would shadow it. Must stay qualified.
        let mut s = scope(&["Inner"]);
        assert_eq!(
            s.resolve("super::super::super::serde::Inner").to_string(),
            "super :: super :: super :: serde :: Inner"
        );
        assert!(s.use_items().is_empty());
    }

    #[test]
    fn parent_rung_works_for_extern_paths() {
        let mut s = scope(&["Foo"]);
        assert_eq!(s.resolve("::ext::sub::Foo").to_string(), "sub :: Foo");
        assert_eq!(s.use_items().to_string(), "use :: ext :: sub ;");
    }

    #[test]
    fn parent_collision_falls_fully_qualified() {
        let mut s = scope(&["Inner", "outer_b"]);
        assert_eq!(
            s.resolve("super::super::super::outer_b::Inner").to_string(),
            "super :: super :: super :: outer_b :: Inner"
        );
        assert!(s.use_items().is_empty());
    }

    #[test]
    fn same_path_resolves_consistently() {
        let mut s = scope(&[]);
        let first = s.resolve("::ext::Foo").to_string();
        let second = s.resolve("::ext::Foo").to_string();
        assert_eq!(first, second);
        assert_eq!(s.use_items().to_string(), "use :: ext :: Foo ;");
    }

    #[test]
    fn keyword_segments_survive() {
        // Paths arrive unescaped (`type`, not `r#type`); escaping happens in
        // rust_path_to_tokens. The leaf binds bare and the recorded use path
        // gets the raw-ident escaping.
        let mut s = scope(&[]);
        assert_eq!(
            s.resolve("super::super::super::type::LatLng").to_string(),
            "LatLng"
        );
        assert_eq!(
            s.use_items().to_string(),
            "use super :: super :: super :: r#type :: LatLng ;"
        );
    }

    #[test]
    fn use_items_are_sorted_by_short_name() {
        let mut s = scope(&[]);
        s.resolve("::ext::Zebra");
        s.resolve("::ext::Alpha");
        assert_eq!(
            s.use_items().to_string(),
            "use :: ext :: Alpha ; use :: ext :: Zebra ;"
        );
    }

    #[test]
    fn bare_path_is_passthrough() {
        let mut s = scope(&[]);
        assert_eq!(s.resolve("Foo").to_string(), "Foo");
        assert!(s.use_items().is_empty());
    }
}
