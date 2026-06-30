# Changelog

This project follows [Semantic Versioning](https://semver.org/). Pre-1.0, a
bump of the minor version may carry breaking changes.

## 0.3.0

Makes `cover` pointcuts model _effective_ visibility — reachability from the
crate root — so the surface a requirement seals matches the surface the crate
actually exposes, and `pub use` re-exports can no longer hide a back door from
the census.

### Added

- **Effective visibility.** A pointcut's `pub`/`pub(crate)` predicate now means
  reachability from the crate root, not the syntactic token at the definition
  site. A `pub fn` sealed behind a private module is no longer part of a
  `pub within` surface; `any` still covers items regardless of reachability.
- **Re-export tracking.** A `pub use` re-export is surfaced as an item at its
  re-exported path, with the seal bound to the target's content. A `pub fn`
  buried in a private module and re-exported into the public API is caught at
  the re-exported path, where syntactic visibility alone missed it. Renames,
  groups, `self`/`super`/`crate` roots, globs (`pub use foo::*`), and chained
  re-exports are resolved within the crate; `#[path]` modules and
  macro-generated `use` are not modeled.
- **`ExternalReexport` finding.** A `pub use` of an out-of-crate item enters the
  surface as an unsealable entry — its body lives in another crate — and is
  reported until acknowledged by an `accept`, so no external entry point slips
  in unnoticed.

### Changed

- **Breaking (`rubric-trace` library):** `ItemFacts` gains `external_reexport`,
  and its `vis` now carries _effective_ (not syntactic) visibility after the new
  `reach` pass; `Visibility` derives `Ord`. `Finding` gains `ExternalReexport`.
  A new `pub mod reach` exposes `ReexportEdge` and `lower`, the pure derive pass
  the scanner runs before the oracle.
- The meaning of an unchanged `cover = "pub within …"` may shift: items
  reachable only through a private module leave the surface, and re-exported
  items enter it.

### Migration

Existing projects stay green until they opt into `cover`. Where they do, run
`cargo rubric accept` once after upgrading: a pointcut may now report
re-exported items as uncovered and private-module items as dropped coverage. For
`reconcile` requirements, run `cargo rubric attest` to re-seed the root. To
cover items regardless of reachability, use the `any` predicate.

## 0.2.0

Adds aspect-oriented coverage and attestation on top of the 0.1.x attribution
chain, plus a temporal audit command.

### Added

- **Seal modes.** A requirement can set `seal = "body"` (default), `"signature"`,
  `"full"` (signature and body), or `"off"`. The `signature` and `full` modes
  cover visibility. A `fn` turning `pub fn` trips the seal.
- **Pointcuts (`cover`).** A requirement can bind a whole set of items with a
  `cover = "<vis> [<kind>] within <scope>"` designator instead of one annotation
  per item. Matching items are bound as satisfiers and sealed.
- **Census.** An item a `cover` pointcut matches but has no seal yet (for example
  a new `pub`) is reported as uncovered until accepted.
- **Content-sealed external evidence.** `external:` paths are read and sealed by
  their file bytes under the `file:` scheme, and a missing file is reported as
  unresolved.
- **Attestation and reconciliation.** A requirement with `reconcile = true`
  records an attestation root under `<attest>`. `cargo rubric attest` writes it.
  `cargo rubric accept` does not. A re-`accept` that moves a leg stays
  unreconciled until reviewed and re-attested.
- **Temporal audit.** `cargo rubric audit` walks the git history of `rubric.lock`
  and flags commits where a `reconcile` leg moved without its `<attest>` root
  moving in the same commit.

### Changed

- **Breaking (`rubric-trace` library):** `Requirement.sig_only: bool` is replaced
  by `Requirement.seal: SealMode`. `ItemFacts` gains `vis`, `kind`, `signature`,
  and `evidence_seal`. `Finding` gains `Uncovered` and `Unreconciled`.
- The `sig_only = true` manifest key still parses, mapped to `seal = "off"`.
  Existing `rubric.toml` files keep working.

### Fixed

A pre-release review pass over the 0.2.0 feature set. The reasoning lives here
because the commits themselves are one line each.

- **Scanner: an unbalanced `<` no longer eats the file.** A `const`/`static`/`type`
  or tuple struct whose value held a `<<` shift or a `<` comparison ran the item
  scan to end of file, silently dropping every later item and its annotations. The
  terminating `;` now ignores angle-bracket depth, which a value's operators can
  inflate but a generic list never spans.
- **Scanner: `static mut NAME` is named correctly**, rather than recording the
  static under `crate::mut`.
- **Audit: sound leg comparison.** A reconcile leg *removed* in a commit now counts
  as moved, and a *deleted* `<attest>` root no longer reads as a re-attestation.
  Each had let a blind accept slip past.
- **Audit: range scope.** `cargo rubric audit <since>` walks only `<since>..HEAD`,
  judging a branch on its own commits; the no-arg form stays the full-history
  forensic report. Same-commit attestation is the working norm.
- **Oracle: loud mismatches instead of silent drops.** A `satisfies` annotation on
  a non-function item, a body seal on an item with no body, and a `signature`/`full`
  seal whose every leg is external evidence are now reported rather than dropped or
  sealed by existence. A cover member demoted out of its pointcut is reported as
  dropped coverage rather than vanishing on the next accept.
- **Pointcut: a stray `*`** (e.g. `crate::api::**`) is rejected instead of parsing
  and matching nothing.
- **Accept** no longer carries forward attestation roots for removed or
  de-reconciled requirements.
- **Smaller:** the unreconciled message no longer points at `attest` before other
  findings clear; the item index is built once per check; a dead match arm is gone.

The new `Finding` variants (`MisplacedAnnotation`, `SealModeMismatch`,
`SealModeOnExternal`, `CoverageDropped`) and the changed `attestation_root`
signature are additive-but-breaking for the pre-1.0 `rubric-trace` library.

### Migration

Existing projects stay green. The new checks fire only for requirements that opt
in with `cover` or `reconcile`. After upgrading, run `cargo rubric accept` once to
regenerate `rubric.lock` with the new schemes. For any requirement you set to
`reconcile = true`, run `cargo rubric attest` once to seed its root.

## 0.1.2

The attribution chain: `(requirement, satisfier, verifier)` with `stmt:`/`body:`
seals, the `check`/`accept`/`trace`/`log` commands, and three annotation forms.
