# rubric

Rubric is a requirements traceability tool for Rust. You write down what the crate is meant to do as requirements in a `rubric.toml` file, then annotate the function that satisfies each requirement and the test that verifies it. Rubric records those links and watches them. When a requirement, its function, or its test changes, rubric flags the build, and it stays flagged until someone reviews the change and accepts it.

## What rubric solves

A Rust crate typically involves three artifacts: documentation, application code, and tests. In some projects (safety-critical firmware, regulated systems, anything with a contractual spec) these three are *required* to stay in agreement. In most others, the team *wants* them to. Rubric is built for both. Teams that are required to keep these in agreement get a mechanism for it. Teams that just want to get a reason to.

```
               [ requirements ]
                  /         \
                 v           v
[ application code ] <----> [ testing code ]
```

Rustc and cargo already couple code and tests. Tests `use` the items they exercise, and `cargo test` produces behavioral evidence. But that evidence is anonymous. The toolchain doesn't know which test corresponds to which requirement, and nothing structural ties a requirement to the code that realizes it. Keeping a requirement in agreement with its code and its tests falls to the developer, and the build doesn't track it.

Rubric names the couplings that already exist. The toolchain can then check they are still intact.

## The model

Rubric tracks one thing, the **attribution chain**. It is a triplet of

> (requirement, satisfier, verifier)

where the requirement is a labeled statement in `rubric.toml`, the satisfier is the function that realizes it, and the verifier is the test that demonstrates it. All three legs are **sealed**, or content-hashed. A change to the requirement's wording, the satisfier's body, or the verifier's body breaks the seal and fails `cargo rubric check` until the change is accepted.

## Quick start

```bash
cargo install cargo-rubric
cd path/to/your/crate
cargo rubric init
```

Define a requirement in `rubric.toml`:

```toml
[req.VOTER-1]
kind = "functional"
statement = "Two identical inputs always out-vote the third"
```

Annotate the code and the test. A plain comment is enough:

```rust
// satisfies: VOTER-1
pub fn vote(a: bool, b: bool, c: bool) -> bool {
    (a & b) | (b & c) | (a & c)
}

#[cfg(test)]
mod tests {
    use super::*;

    // verifies: VOTER-1
    #[test]
    fn two_against_one() {
        assert!(vote(true, true, false));
        assert!(!vote(false, false, true));
    }
}
```

Accept the chain, then verify:

```bash
cargo rubric accept   # records the chain and seals all three legs into rubric.lock
cargo rubric check    # green, run this in CI
```

From now on, editing `vote`'s body, gutting the test, or rewording the statement makes `check` fail, naming the requirement:

```
✗ VOTER-1 "Two identical inputs always out-vote the third"
    crate::vote changed since last accept
```

Findings name the requirement and the cited item whose content moved. A
reformat that changes no tokens leaves the seal intact and `check` green.

Running `cargo rubric accept` re-seals, and the resulting `rubric.lock` diff rides in the pull request. PR review is the attestation step, the same social contract as `Cargo.lock`.

## Two files

- **`rubric.toml`**: the human-authored contract. Requirement definitions (label, kind, statement), plus optional `satisfied_by` / `verified_by` declarations for anything the scanner can't reach: integration tests under `tests/`, bin-only crates, external evidence (`external:docs/lab-report-7.pdf`).
- **`rubric.lock`**: machine-managed. The attribution chain plus the seal for each leg. Written by `accept`, read by `check`, reviewed in PRs like `Cargo.lock`. Each entry records its origin. The scanner owns *annotation* entries and adds or removes them as annotations come and go. *Declared* entries mirror `rubric.toml`, and a scan leaves them in place.

There's no third file. History and forensics come from version control. See `cargo rubric log` below.

## The seal

Seals are scheme-tagged content hashes (FNV-1a 64 over normalized tokens), one per chain leg:

- **`stmt:`** seals the requirement's statement text. Quietly softening "must reject" to "should reject" breaks the chain like any code change.
- **`body:`** seals the satisfier's body, with whitespace and comments stripped. Reformatting doesn't break it. Any token change does.
- **`body:`** seals the verifier's body the same way. A gutted or deleted test can no longer vouch for a requirement.

Token hashes break on refactors that don't change behavior, like renaming a local or reordering independent statements. This is deliberate. In the contexts rubric serves, the two mistakes don't cost the same. A real change that slips past the seal can surface much later as an audit finding. A false alarm only asks you to run `accept` and glance at a diff line that already names the requirement. So rubric over-reports on purpose. Requirements with low tolerance for that noise can set `sig_only = true`, which tracks cited items by existence only.

## Git is the ledger

`cargo rubric log` walks the git history of `rubric.lock` and renders the seal-event timeline: when each `(requirement, item)` re-sealed, the hash transition, the commit, and the author, with the actual source diff one `git show` away. Rubric keeps no history file of its own, so there's no second format to learn and nothing to grow unbounded or compact. If your history is gone, the source diffs that timeline would point at are gone too.

## Annotation forms

An annotation can be written in three ways, shown in the table below. They differ only in what they require, and the scanner reads all three into the same chain data. You can mix them within a single crate.

| Form | Spelling | Requires |
|---|---|---|
| Comment | `// satisfies: VOTER-1` | Nothing |
| `cfg_attr` | `#[cfg_attr(any(), satisfies(VOTER-1))]` | Nothing (compiler-inert) |
| Bare attribute | `#[satisfies(VOTER-1)]` | `rubric-trace-macros` (identity pass-through, ~15 lines) |

`cfg_attr(any(), ...)`: `any()` with no arguments is always false, so the compiler discards the attribute without resolving `satisfies`. No proc-macro, no dependency, no runtime effect. The comment form is more inert still. There's nothing for the compiler to see at all.

The same applies to `verifies:` / `#[verifies(...)]` on tests. For items no annotation can reach, declare the path directly in `rubric.toml`.

Labels are bare requirement IDs (`VOTER-1`, `PARSER-3`), the convention requirements documents already use. There's no module-path namespace and no generated `reqs` module. A typo in a label is caught by `check` as an orphan annotation. The compiler no longer catches it.

Markers live in source text and are present in all builds. Nothing is injected at compile time and nothing needs compiling out. The same source tree yields the same `rubric.lock` on any machine, toolchain, or build profile.

## What `check` verifies

`check` runs each of these as a pure function over `(source, manifest, attribution data)`:

1. Every functional requirement has at least one cited satisfier.
2. Every requirement (functional or invariant) has at least one cited verifier.
3. Every cited item path resolves to a real item in the source tree.
4. Every seal matches its current content: statement, satisfier, and verifier.
5. Every cited verifier is live: the test exists and isn't `#[ignore]`d.
6. No orphan annotations (label not defined in the manifest).
7. No kind violations: a `satisfies` annotation on an `invariant` requirement, which has no satisfying function.

## Commands

| Command | What it does |
|---|---|
| `cargo rubric init` | Scaffold `rubric.toml` in a crate or workspace member |
| `cargo rubric check` | Read-only oracle verdict, non-zero exit on any finding. Run in CI. |
| `cargo rubric accept` | Scan annotations and re-seal the chain in one motion. Prints what changed |
| `cargo rubric trace` | Render the traceability matrix as a standalone markdown report |
| `cargo rubric log` | Seal-event timeline from the git history of `rubric.lock` |

Of these, two are the ones you run day to day. You run `check` to find out whether the chain is still intact, and `accept` to record a change you meant to make. The matrix from `trace` is a self-contained artifact, suitable for an evidence package. Teams that want it in rustdoc can `include_str!` it into a doc module.

## Safety-critical use

The properties that matter for qualification:

- **Zero rubric dependencies in the consumer's tree.** With the comment or `cfg_attr` forms, no rubric crate appears in `Cargo.toml` at all. There is nothing to qualify in the build graph.
- **The oracle is standalone.** `cargo rubric check` runs against a source tree without compiling it or expanding any macro. The trust surface is one auditable binary.
- **The core is pure.** Verification logic in `rubric-trace` is pure functions: no I/O, no globals, no side effects. If an auditor needs a formal treatment, this core is the single target.
- **Lexing uses the compiler's own lexer.** The scanner uses `rustc_lexer`, the same lexer rustc uses. It lives in the tool and never enters your build graph.
- **The core tracks itself.** These properties aren't only asserted here. Rubric's pure core carries its own [`rubric.toml`](crates/rubric-trace/rubric.toml): the seal mechanism and every clause of the oracle's check set are requirements, each sealed to the function that realizes it and the test that demonstrates it. `cargo rubric check` runs green on this repository, so a change to the core that slips its test trips the same chain everyone else relies on.

## Crates

| Crate | Role | Needed by consumers? |
|---|---|---|
| `cargo-rubric` | CLI + source scanner | No, a standalone tool |
| `rubric-trace` | Pure verification core | Optional (planned `build.rs` integration) |
| `rubric-trace-macros` | Identity proc-macros for the bare-attribute form | Optional, comment and `cfg_attr` forms need nothing |

Build-script integration is a planned convenience over the same core. A `build.rs` would call `rubric-trace` to surface `check` findings as `cargo:warning` lines during a normal build. It isn't implemented yet. The standalone oracle is the path of record.

## Status

Published on crates.io. All five commands run, including across the members of a Cargo workspace. The pure core in `rubric-trace` is stdlib-only. The scanner and all I/O live in `cargo-rubric`.

Known gaps: `external:` evidence paths aren't yet existence-checked, the `build.rs` face isn't built, and items inside closures or macro bodies aren't scanned (declare those in `rubric.toml`). Annotations in `src/` and in `tests/` (including nested test files and shared `mod` helpers) are scanned.

## Contributing

Rubric started from [a podcast episode on traceability](https://music.youtube.com/watch?v=-f6RM7fVPvE&si=j_kM_rOyjrpNhYyA) discussing gaps in the Rust ecosystem for software traceability and document maintenance. The project is early-stage and all contributions are welcome.

## License

Dual-licensed under either of:

- Apache License, Version 2.0 (LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license (LICENSE-MIT or http://opensource.org/licenses/MIT)

at your option.
