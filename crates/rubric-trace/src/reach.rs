//! Effective visibility and re-export derivation.
//!
//! The scanner records the `pub` or `pub(crate)` token it finds at each
//! definition, but that token doesn't tell the whole story. A `pub fn`
//! inside a private module reads as public where it's written, yet it isn't
//! reachable from the crate root. This pass runs before any pointcut and
//! corrects two things.
//!
//! First, effective visibility. Each item's own visibility is compared with
//! every module that encloses it, and the least visible link in that chain
//! wins.
//! A `pub fn` under a private module comes out private, so a
//! `cover = "pub fn within ..."` census leaves it alone.
//!
//! Second, re-export aliases. `pub use internal::backdoor;` puts an item on
//! the public surface under a new path. For each in-crate `pub use`, this
//! pass adds an alias at that path and copies the target's content onto it.
//! Sealing the alias then seals the same content the target holds, and a
//! pointcut finds it where it was re-exported. Everything downstream treats
//! the alias as an ordinary item.
//!
//! A `pub use` that points out of the crate (`pub use serde::Serialize;`)
//! can't be resolved here, since the target lives in another crate. Those
//! are handed to the `external_reexport` finding instead of being dropped.
//!
//! The pass is pure and stdlib-only, like the rest of the core. If an
//! enclosing module is missing from the item set (say a `#[path]`-relocated
//! module the path model can't reconstruct), the item is treated as fully
//! visible rather than private. That way the census errs toward calling an
//! item uncovered, which is loud, instead of quietly dropping it.

use std::collections::{BTreeMap, BTreeSet};

use crate::check::{ItemFacts, ItemKind, Visibility};

/// A `pub use` re-export edge captured by the scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReexportEdge {
    /// Module the `pub use` appears in, e.g. `["crate", "api"]`.
    pub in_module: Vec<String>,
    /// The use-path segments as written, relative to `in_module`. May start
    /// with `crate`/`self`/`super` (in-crate) or be a bare/extern path.
    /// For a glob (`a::b::*`) this is the parent path `["a","b"]`.
    pub path: Vec<String>,
    /// The exported name (the `as` rename or the leaf segment). Empty for a
    /// glob, whose children name themselves.
    pub alias: String,
    /// Visibility of the `use` itself (`Pub` or `PubCrate`).
    pub vis: Visibility,
    /// `pub use a::b::*` re-exports every effective-public child of `a::b`.
    pub glob: bool,
}

/// Rewrite each item's `vis` to its effective visibility and inject an alias
/// item for every resolvable in-crate `pub use` re-export.
// satisfies: REACH-VIS, REACH-REEXPORT
pub fn lower(items: Vec<ItemFacts>, edges: &[ReexportEdge]) -> Vec<ItemFacts> {
    // Reduce the real items to effective visibility first, so glob-child
    // selection and chain resolution see the corrected values.
    let reals = fold_effective_vis(items);
    let aliases = synth_aliases(&reals, edges);

    let mut map: BTreeMap<String, ItemFacts> =
        reals.into_iter().map(|i| (i.path.clone(), i)).collect();
    // `synth_aliases` has already applied explicit-over-glob precedence and
    // skipped any path that collides with a real item.
    for a in aliases {
        map.insert(a.path.clone(), a);
    }

    // Run the visibility reduction once more over the combined set. A
    // `pub use` written inside a private module is itself unreachable, so
    // this second pass limits each alias to the visibility of the modules it
    // lands in. It does nothing to the real items.
    fold_effective_vis(map.into_values().collect())
}

/// Rewrite `vis` to effective visibility for every `crate`-rooted item.
fn fold_effective_vis(mut items: Vec<ItemFacts>) -> Vec<ItemFacts> {
    let index: BTreeMap<String, Visibility> =
        items.iter().map(|i| (i.path.clone(), i.vis)).collect();
    for it in &mut items {
        // The `tests` tree is a separate compilation, never crate API surface.
        if it.path == "crate" || it.path.starts_with("crate::") {
            it.vis = effective_vis(&it.path, it.vis, &index);
        }
    }
    items
}

/// Effective visibility of `path`: the minimum of its own `syntactic`
/// visibility and every enclosing container's. An enclosing segment absent
/// from `index` is skipped (treated as fully visible).
fn effective_vis(
    path: &str,
    syntactic: Visibility,
    index: &BTreeMap<String, Visibility>,
) -> Visibility {
    let segs: Vec<&str> = path.split("::").collect();
    let mut eff = syntactic;
    // Proper ancestors only: `crate::a::b::x` folds `crate`, `crate::a`,
    // `crate::a::b`. The crate root carries no item and is the public ceiling.
    for i in 1..segs.len() {
        let prefix = segs[..i].join("::");
        if let Some(&enclosing) = index.get(&prefix) {
            eff = eff.min(enclosing);
        }
    }
    eff
}

/// Synthesize alias items for every resolvable in-crate `pub use` edge.
/// Glob aliases are produced first and explicit aliases second, so an
/// explicit re-export overrides a glob on a name collision (Rust precedence).
fn synth_aliases(reals: &[ItemFacts], edges: &[ReexportEdge]) -> Vec<ItemFacts> {
    let by_path: BTreeMap<&str, &ItemFacts> =
        reals.iter().map(|i| (i.path.as_str(), i)).collect();

    // Concrete requests: (alias_path, target_abs_path, vis, from_glob).
    let mut requests: Vec<(String, String, Visibility, bool)> = Vec::new();
    // Immediate alias→target map, for following chained re-exports.
    let mut alias_target: BTreeMap<String, String> = BTreeMap::new();
    // Out-of-crate re-exports: (alias_path, vis). Unsealable, flagged.
    let mut externals: Vec<(String, Visibility)> = Vec::new();

    for e in edges {
        let Some(abs) = resolve_root(&e.path, &e.in_module) else {
            // A bare/extern non-glob target is an out-of-crate re-export. A
            // glob over an extern path can't be enumerated, so it is dropped.
            if !e.glob {
                externals.push((join_seg(&e.in_module, &e.alias), e.vis));
            }
            continue;
        };
        if e.glob {
            for child in effective_public_children(&abs, &by_path, e.vis) {
                let alias_path = join_seg(&e.in_module, last_seg(&child));
                requests.push((alias_path, child, e.vis, true));
            }
        } else {
            let alias_path = join_seg(&e.in_module, &e.alias);
            alias_target.insert(alias_path.clone(), abs.clone());
            requests.push((alias_path, abs, e.vis, false));
        }
    }

    // Glob first, then explicit. On a path collision the later insert wins.
    let mut out: BTreeMap<String, ItemFacts> = BTreeMap::new();
    for glob_pass in [true, false] {
        for (alias_path, target_abs, vis, from_glob) in &requests {
            if *from_glob != glob_pass {
                continue;
            }
            if by_path.contains_key(alias_path.as_str()) {
                continue; // never shadow a real item
            }
            if let Some(target) = resolve_real(target_abs, &by_path, &alias_target) {
                out.insert(alias_path.clone(), synth_one(alias_path, target, *vis));
            }
        }
    }
    // External re-exports last: a real item or an in-crate alias of the same
    // name takes precedence over an unsealable external one.
    for (alias_path, vis) in externals {
        if by_path.contains_key(alias_path.as_str()) || out.contains_key(&alias_path) {
            continue;
        }
        out.insert(alias_path.clone(), synth_external(&alias_path, vis));
    }
    out.into_values().collect()
}

/// Resolve a use-path against its importing module to an absolute in-crate
/// path. `None` for a bare/extern path (first segment is not `crate`/`self`/
/// `super`), which the scanner cannot resolve to local content.
fn resolve_root(path: &[String], in_module: &[String]) -> Option<String> {
    let first = path.first()?;
    let (mut abs, rest): (Vec<String>, &[String]) = match first.as_str() {
        "crate" => (vec!["crate".to_string()], &path[1..]),
        "self" => (in_module.to_vec(), &path[1..]),
        "super" => {
            let mut abs = in_module.to_vec();
            let mut i = 0;
            while path.get(i).map(|s| s == "super").unwrap_or(false) {
                abs.pop()?; // can't climb above the crate root
                i += 1;
            }
            (abs, &path[i..])
        }
        _ => return None,
    };
    abs.extend(rest.iter().cloned());
    Some(abs.join("::"))
}

/// Follow an alias-target chain to the first real item, breaking cycles.
fn resolve_real<'a>(
    target: &str,
    by_path: &BTreeMap<&str, &'a ItemFacts>,
    alias_target: &BTreeMap<String, String>,
) -> Option<&'a ItemFacts> {
    let mut cur = target.to_string();
    let mut seen = BTreeSet::new();
    loop {
        if let Some(&it) = by_path.get(cur.as_str()) {
            return Some(it);
        }
        if !seen.insert(cur.clone()) {
            return None; // cycle
        }
        cur = alias_target.get(&cur)?.clone();
    }
}

/// Direct children of `module` that are public at or above `min_vis`.
fn effective_public_children(
    module: &str,
    by_path: &BTreeMap<&str, &ItemFacts>,
    min_vis: Visibility,
) -> Vec<String> {
    let prefix = format!("{module}::");
    by_path
        .iter()
        .filter(|(p, it)| {
            p.starts_with(&prefix) && !p[prefix.len()..].contains("::") && it.vis >= min_vis
        })
        .map(|(p, _)| p.to_string())
        .collect()
}

/// Build an alias `ItemFacts` that clones the target's content. Its `vis`
/// starts as the re-export's. `lower`'s second fold caps it by its location.
fn synth_one(alias_path: &str, target: &ItemFacts, vis: Visibility) -> ItemFacts {
    ItemFacts {
        path: alias_path.to_string(),
        resolved: true,
        is_test: false,
        is_ignored: false,
        vis,
        kind: target.kind,
        body: target.body.clone(),
        signature: target.signature.clone(),
        evidence_seal: None,
        external_reexport: false,
    }
}

/// Build an alias for a `pub use` of an out-of-crate item. Its body lives in
/// another crate, so there is nothing to seal. It carries no content and is
/// flagged for the `ExternalReexport` finding. The `kind` is just a
/// placeholder. A cross-crate item's kind isn't known here, so the pointcut
/// skips the kind filter for flagged aliases.
fn synth_external(alias_path: &str, vis: Visibility) -> ItemFacts {
    ItemFacts {
        path: alias_path.to_string(),
        resolved: true,
        is_test: false,
        is_ignored: false,
        vis,
        kind: ItemKind::Fn,
        body: None,
        signature: None,
        evidence_seal: None,
        external_reexport: true,
    }
}

fn last_seg(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

fn join_seg(module: &[String], name: &str) -> String {
    let mut segs = module.to_vec();
    segs.push(name.to_string());
    segs.join("::")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::ItemKind;

    fn it(path: &str, vis: Visibility, kind: ItemKind) -> ItemFacts {
        ItemFacts {
            path: path.into(),
            resolved: true,
            is_test: false,
            is_ignored: false,
            vis,
            kind,
            body: Some(format!("body of {path}")),
            signature: Some(format!("sig of {path}")),
            evidence_seal: None,
            external_reexport: false,
        }
    }

    fn edge(in_module: &str, path: &str, alias: &str, vis: Visibility, glob: bool) -> ReexportEdge {
        ReexportEdge {
            in_module: split(in_module),
            path: split(path),
            alias: alias.into(),
            vis,
            glob,
        }
    }

    fn split(s: &str) -> Vec<String> {
        if s.is_empty() {
            vec![]
        } else {
            s.split("::").map(String::from).collect()
        }
    }

    fn find<'a>(items: &'a [ItemFacts], path: &str) -> Option<&'a ItemFacts> {
        items.iter().find(|i| i.path == path)
    }

    // --- effective-visibility fold (Slice 1) ---

    fn eff_of(items: Vec<ItemFacts>, path: &str) -> Visibility {
        find(&lower(items, &[]), path).expect("item present").vis
    }

    // verifies: REACH-VIS
    #[test]
    fn pub_fn_in_private_mod_is_lowered() {
        let items = vec![
            it("crate::internal", Visibility::Private, ItemKind::Mod),
            it("crate::internal::x", Visibility::Pub, ItemKind::Fn),
        ];
        assert_eq!(eff_of(items, "crate::internal::x"), Visibility::Private);
    }

    #[test]
    fn pub_fn_in_pub_mod_stays_pub() {
        let items = vec![
            it("crate::api", Visibility::Pub, ItemKind::Mod),
            it("crate::api::x", Visibility::Pub, ItemKind::Fn),
        ];
        assert_eq!(eff_of(items, "crate::api::x"), Visibility::Pub);
    }

    #[test]
    fn pub_crate_mod_caps_pub_fn() {
        let items = vec![
            it("crate::m", Visibility::PubCrate, ItemKind::Mod),
            it("crate::m::x", Visibility::Pub, ItemKind::Fn),
        ];
        assert_eq!(eff_of(items, "crate::m::x"), Visibility::PubCrate);
    }

    #[test]
    fn least_visible_link_in_a_nested_chain_wins() {
        let items = vec![
            it("crate::a", Visibility::Pub, ItemKind::Mod),
            it("crate::a::b", Visibility::Private, ItemKind::Mod),
            it("crate::a::b::x", Visibility::Pub, ItemKind::Fn),
        ];
        assert_eq!(eff_of(items, "crate::a::b::x"), Visibility::Private);
    }

    #[test]
    fn crate_root_pub_fn_has_no_enclosing_cap() {
        let items = vec![it("crate::x", Visibility::Pub, ItemKind::Fn)];
        assert_eq!(eff_of(items, "crate::x"), Visibility::Pub);
    }

    #[test]
    fn method_is_capped_by_its_modules_and_its_type() {
        let under_priv = vec![
            it("crate::internal", Visibility::Private, ItemKind::Mod),
            it("crate::internal::S", Visibility::Pub, ItemKind::Struct),
            it("crate::internal::S::m", Visibility::Pub, ItemKind::Fn),
        ];
        assert_eq!(eff_of(under_priv, "crate::internal::S::m"), Visibility::Private);

        let capped_by_type = vec![
            it("crate::S", Visibility::PubCrate, ItemKind::Struct),
            it("crate::S::m", Visibility::Pub, ItemKind::Fn),
        ];
        assert_eq!(eff_of(capped_by_type, "crate::S::m"), Visibility::PubCrate);
    }

    #[test]
    fn missing_enclosing_segment_does_not_lower() {
        let items = vec![it("crate::ghost::x", Visibility::Pub, ItemKind::Fn)];
        assert_eq!(eff_of(items, "crate::ghost::x"), Visibility::Pub);
    }

    #[test]
    fn tests_tree_is_untouched() {
        let items = vec![
            it("tests::common", Visibility::Private, ItemKind::Mod),
            it("tests::common::helper", Visibility::Pub, ItemKind::Fn),
        ];
        assert_eq!(eff_of(items, "tests::common::helper"), Visibility::Pub);
    }

    // --- re-export alias synthesis (Slice 2) ---

    // verifies: REACH-REEXPORT
    #[test]
    fn reexport_of_private_module_item_creates_public_alias() {
        // The crux: a fn private at its def site, re-exported at crate root.
        let items = vec![
            it("crate::internal", Visibility::Private, ItemKind::Mod),
            it("crate::internal::backdoor", Visibility::Pub, ItemKind::Fn),
        ];
        let edges = vec![edge("crate", "crate::internal::backdoor", "backdoor", Visibility::Pub, false)];
        let out = lower(items, &edges);

        // Def-site copy demoted to private (not double-counted by a pub census).
        assert_eq!(find(&out, "crate::internal::backdoor").unwrap().vis, Visibility::Private);
        // Alias at the re-exported path is public and seals the target's body.
        let alias = find(&out, "crate::backdoor").expect("alias present");
        assert_eq!(alias.vis, Visibility::Pub);
        assert_eq!(alias.body.as_deref(), Some("body of crate::internal::backdoor"));
        assert_eq!(alias.kind, ItemKind::Fn);
    }

    #[test]
    fn rename_uses_the_aliased_name_for_the_path() {
        let items = vec![it("crate::internal::backdoor", Visibility::Pub, ItemKind::Fn)];
        let edges = vec![edge("crate", "crate::internal::backdoor", "front", Visibility::Pub, false)];
        let out = lower(items, &edges);
        assert!(find(&out, "crate::front").is_some());
        assert!(find(&out, "crate::backdoor").is_none());
    }

    #[test]
    fn self_and_super_roots_resolve() {
        let items = vec![
            it("crate::api::sub::thing", Visibility::Pub, ItemKind::Fn),
            it("crate::other::sibling", Visibility::Pub, ItemKind::Fn),
        ];
        let edges = vec![
            // `pub use self::sub::thing;` inside crate::api
            edge("crate::api", "self::sub::thing", "thing", Visibility::Pub, false),
            // `pub use super::other::sibling;` inside crate::api
            edge("crate::api", "super::other::sibling", "sibling", Visibility::Pub, false),
        ];
        let out = lower(items, &edges);
        assert!(find(&out, "crate::api::thing").is_some());
        assert!(find(&out, "crate::api::sibling").is_some());
    }

    #[test]
    fn glob_aliases_every_public_child_only() {
        let items = vec![
            it("crate::sim", Visibility::Pub, ItemKind::Mod),
            it("crate::sim::a", Visibility::Pub, ItemKind::Fn),
            it("crate::sim::b", Visibility::Pub, ItemKind::Fn),
            it("crate::sim::hidden", Visibility::Private, ItemKind::Fn),
            it("crate::sim::deep::c", Visibility::Pub, ItemKind::Fn), // not a direct child
        ];
        let edges = vec![edge("crate", "crate::sim", "", Visibility::Pub, true)];
        let out = lower(items, &edges);
        assert!(find(&out, "crate::a").is_some());
        assert!(find(&out, "crate::b").is_some());
        assert!(find(&out, "crate::hidden").is_none()); // private child excluded
        assert!(find(&out, "crate::c").is_none()); // not a direct child
    }

    #[test]
    fn explicit_reexport_wins_over_glob_on_collision() {
        let items = vec![
            it("crate::sim::a", Visibility::Pub, ItemKind::Fn),
            it("crate::other::a", Visibility::Pub, ItemKind::Fn),
        ];
        let edges = vec![
            edge("crate", "crate::sim", "", Visibility::Pub, true), // glob brings sim::a as crate::a
            edge("crate", "crate::other::a", "a", Visibility::Pub, false), // explicit crate::a
        ];
        let out = lower(items, &edges);
        // The explicit re-export's target content wins.
        assert_eq!(
            find(&out, "crate::a").unwrap().body.as_deref(),
            Some("body of crate::other::a"),
        );
    }

    #[test]
    fn chained_reexport_resolves_to_the_real_item() {
        let items = vec![it("crate::deep::real", Visibility::Pub, ItemKind::Fn)];
        let edges = vec![
            // crate::mid::real := crate::deep::real
            edge("crate::mid", "crate::deep::real", "real", Visibility::Pub, false),
            // crate::real := crate::mid::real (an alias)
            edge("crate", "crate::mid::real", "real", Visibility::Pub, false),
        ];
        let out = lower(items, &edges);
        let alias = find(&out, "crate::real").expect("chained alias present");
        assert_eq!(alias.body.as_deref(), Some("body of crate::deep::real"));
    }

    #[test]
    fn reexport_cycle_terminates_without_an_alias() {
        // a := b and b := a, neither resolving to a real item.
        let edges = vec![
            edge("crate", "crate::b", "a", Visibility::Pub, false),
            edge("crate", "crate::a", "b", Visibility::Pub, false),
        ];
        let out = lower(vec![], &edges);
        assert!(find(&out, "crate::a").is_none());
        assert!(find(&out, "crate::b").is_none());
    }

    // verifies: REACH-REEXPORT
    #[test]
    fn extern_reexport_becomes_a_flagged_external_alias() {
        // A bare first segment (not crate/self/super) means the target is
        // out of crate. The alias is unsealable and gets flagged for the
        // ExternalReexport finding.
        let edges = vec![edge("crate", "serde::Serialize", "Serialize", Visibility::Pub, false)];
        let out = lower(vec![], &edges);
        let alias = find(&out, "crate::Serialize").expect("external alias present");
        assert!(alias.external_reexport);
        assert!(alias.body.is_none());
        assert_eq!(alias.vis, Visibility::Pub);
    }

    #[test]
    fn a_real_item_takes_precedence_over_an_external_reexport() {
        // If a real item already occupies the alias path, the external one is
        // dropped rather than shadowing it.
        let items = vec![it("crate::Serialize", Visibility::Pub, ItemKind::Struct)];
        let edges = vec![edge("crate", "serde::Serialize", "Serialize", Visibility::Pub, false)];
        let out = lower(items, &edges);
        assert!(!find(&out, "crate::Serialize").unwrap().external_reexport);
    }

    #[test]
    fn pub_crate_reexport_alias_is_pub_crate() {
        let items = vec![it("crate::internal::x", Visibility::Pub, ItemKind::Fn)];
        let edges = vec![edge("crate", "crate::internal::x", "x", Visibility::PubCrate, false)];
        let out = lower(items, &edges);
        assert_eq!(find(&out, "crate::x").unwrap().vis, Visibility::PubCrate);
    }

    #[test]
    fn reexport_into_a_private_module_is_capped() {
        // `pub use` inside a private module is itself unreachable.
        let items = vec![
            it("crate::priv", Visibility::Private, ItemKind::Mod),
            it("crate::real", Visibility::Pub, ItemKind::Fn),
        ];
        let edges = vec![edge("crate::priv", "crate::real", "real", Visibility::Pub, false)];
        let out = lower(items, &edges);
        assert_eq!(find(&out, "crate::priv::real").unwrap().vis, Visibility::Private);
    }
}
