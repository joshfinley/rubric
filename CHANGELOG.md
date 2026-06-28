# Changelog

This project follows [Semantic Versioning](https://semver.org/). Pre-1.0, a
bump of the minor version may carry breaking changes.

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
