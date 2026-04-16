# rubric

A requirements traceability tool for Rust crates: declare requirements once, annotate the source that satisfies and the tests that verify them, and let the build pick up drift before it ships.

## What Rubric solves

A Rust crate typically involves three artifacts: documentation, application code, and tests. In some projects — safety-critical firmware, regulated systems, anything with a contractual spec — these three are *required* to stay in agreement. In most others, the team simply *wants* them to. _Rubric_ is built for both: it gives the required-to a mechanism, and the want-to a reason.

A requirement is the unit of agreement worth tracking. It is a behavioral claim about the system — a "the system shall …" statement — that originates outside the code, in a spec, a ticket, a standard, or a product brief, and that the code is meant to realize. Requirements are the subset of documentation that make verifiable assertions.

```
           [ documentation ]
              /         \
             v           v
[ application code ] <----> [ testing code ]
```

Rustc and cargo couple application code and test code twice. At compile time, tests `use` the code they exercise, so renaming or removing an item breaks the build. At runtime, assertions under `cargo test` give behavioral evidence that the code does what the tests expect. The other two edges are different. Nothing structural ties documentation to the code that realizes it, or to the tests that verify it — and yet that coupling *exists*, it just isn't attributed to any particular requirement. When `cargo test` passes, runtime evidence for the underlying claims exists; that's the definition of a test. The evidence is simply anonymous: the toolchain doesn't know which test corresponds to which requirement, so a passing test counts as generic "the code works" rather than "requirement X holds." Agreement along those edges is the developer's discipline, not the build's accounting.

Rubric doesn't invent a new coupling. It adds names to the couplings already carrying traffic, so the toolchain can check the wiring is still intact.

## How it works

_Rubric_ is a cargo subcommand. You declare what the crate must do in a crate-level `rubric.toml`, annotate the functions that satisfy and the tests that verify each requirement, and from the next build onward drift between any of the three artifacts shows up in the normal cargo output — as a warning during development, and as a failure under `--release`.

## Usage

Install the CLI:

```bash
cargo install cargo-rubric
```

In a target crate, scaffold the rubric files:

```bash
cd path/to/your/crate

# Single crate
cargo rubric init

# Single workspace member
cargo rubric init -p <member>

# Full workspace
cargo rubric init --all-members
```

`init` wires Rubric into the crate: it drops the manifest and build script at the crate root, adds the crate and the build-time core as dependencies, and installs the setup macro so requirement names resolve as Rust paths. The precise steps live in the command's source comments.

That's it — Rubric is set up with zero requirements. To add one, define it in `rubric.toml`, annotate the implementing function with `#[satisfies(…)]`, and annotate a verifying test with `#[verifies(…)]`:

```toml
# rubric.toml
[meta]
version = 1

[req.greeter.says_hello]
description = "The greet function returns the string 'hello'"
```

```rust
// src/main.rs

/// Returns "hello".
#[satisfies(crate::reqs::greeter::says_hello)]
pub fn greet() -> &'static str { "hello" }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[verifies(crate::reqs::greeter::says_hello)]
    fn greet_returns_hello() {
        assert_eq!(greet(), "hello");
    }
}
```

> A working example of a simple project using Rubric can be found at [`examples/tmr-voter`](examples/tmr-voter).

Now run a normal build. Because the bodies of those two functions have not yet been sealed, the build script will surface them as warnings — these are not errors, the build still succeeds:

```text
$ cargo build
   Compiling readmedemo v0.1.0 (/tmp/readmedemo)
warning: readmedemo@0.1.0: rubric: function body seal missing: `greeter::says_hello` @ `readmedemo::greet` (/tmp/readmedemo/src/lib.rs:4) — run `cargo rubric seal`
warning: readmedemo@0.1.0: rubric: function body seal missing: `greeter::says_hello` @ `readmedemo::tests::greet_returns_hello` (/tmp/readmedemo/src/lib.rs:11) — run `cargo rubric seal`
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.38s
```

Each annotation registers a *function body seal* — a reviewed snapshot of the annotated function's body, against which future edits are compared. The seal is produced by content-hashing the function's tokens (whitespace, comments, and doc attributes stripped) and recorded in `rubric.lock` at the crate root. The first time Rubric sees a new annotation, it has no seal to compare against; it asks you to seal the current state. Run:

```bash
cargo rubric seal
```

The next build is silent:

```text
$ cargo build
   Compiling readmedemo v0.1.0 (/tmp/readmedemo)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.01s
```

From here on, any edit to `greet`'s body will reappear as a `rubric: function body seal broken: …` warning on the next build, prompting you to review the change and re-seal if the new behavior is correct. This is a deliberate compromise between completeness and iterability: Rubric does not (yet) consume test verification results to decide whether the new behavior still satisfies the requirement, but it will not let the drift pass silently — the developer must seal to dispel the warning. A `cargo build --release` elevates these warnings to build failures, so drift cannot ship in a release artifact unintentionally.

`cargo rubric check` runs the same drift pass on demand without compiling the crate, with more verbose, guided output aimed at onboarding. In a workspace, every command above (`init`, `seal`, `check`) detects a workspace root and iterates each member that has a `rubric.toml`.

`cargo doc` requires no extra flags or configuration. Because the setup macro emits requirement names as real Rust items, they appear in the generated docs alongside the rest of the API. Each annotated function's page shows which requirements it satisfies or verifies, and a top-level `traceability` module links the full matrix. The traceability output is a side effect of the same `cargo doc` the team already runs.

## The traceability matrix

Traceability, in the safety-critical and systems-engineering sense, is the ability to follow a requirement from its origin through every artifact that implements or verifies it. The canonical representation is a matrix: requirements on one axis, implementations and tests on the other, with cells marking the correspondence. Rubric assembles this matrix from its annotations and surfaces it as part of the crate's own rendered output, so the artifact a maintainer already produces (`cargo doc`) doubles as the traceability evidence an auditor would ask for.

`rubric::setup!()` makes requirement names real Rust items, so they appear in `cargo doc`. Each annotated function gains a `# Satisfies requirements` (or `# Verifies requirements`) section listing the linked requirements, rendered as intra-doc links to per-requirement pages. The per-requirement page carries the description from `rubric.toml` plus the stable identity, so a reader can navigate from any annotated item to the requirement that motivates it. A top-level `traceability` module on the crate's docs renders the whole matrix as a single page, with each row's label linking back to its requirement page.

This part is automatic; the consumer adds nothing beyond `rubric::setup!();` and the per-item annotations.

## Current limitations

None of these are blocking for ordinary use; they shape the ceiling.

- **In-tree markdown binding.** The seal system currently tracks drift in annotated Rust functions only. Markdown files in the source tree (`docs/`, crate-level `README.md`, design notes) that reference code items or requirements have no corresponding baseline — if a function changes, the prose that describes it won't trigger a stale warning. Extending the content-hash and seal mechanism to cover items referenced from `.md` files would close the loop on documentation.

- **Cross-crate requirement edges.** A requirement in crate A satisfied by code in crate B is currently out of scope. Supporting it would require a workspace-global label namespace and cross-crate lockfile coordination, which conflicts with the per-crate ownership model that makes workspace support compose cleanly today.

- **Path resolution for exotic generics and macro-emitted items.** The resolver walks source syntactically using `rustc_lexer`. Items behind deeply nested generics or emitted by third-party proc macros can't always be resolved to a qualified path. An `id = "…"` attribute override (see below) would cover these cases manually.

- **Explicit `id = "…"` attribute override.** When automatic path resolution fails — macro-generated items, foreign-language entrypoints, vendored code — the annotation should accept an explicit identity string so the lockfile can still track the item.

- **Profile-aware strictness without `build.rs`.** Strictness (warnings in dev, errors in release) is currently controlled by the build script detecting the profile. Crates that cannot use a build script have no way to vary strictness by profile.

- **Requirements coverage reporting.** The matrix shows which requirements have implementations and tests, but doesn't yet surface the inverse: requirements with no satisfying code or no verifying tests. This is the difference between "here's what's wired up" and "here's what's missing."

- **Richer matrix rendering with diff support.** The current matrix is a snapshot. Showing what changed between two seals — new requirements, dropped coverage, broken seals — would make it useful in review, not just in audit.

- **rustdoc-JSON or librustdoc as an alternative resolver.** The syntactic resolver is fast and dependency-free, but rustdoc's own semantic output would handle generics, trait impls, and re-exports correctly. This could sit behind the existing resolver trait as an opt-in backend for crates that need it.

## Contributing

Rubric started from [a podcast episode on traceability](https://music.youtube.com/watch?v=-f6RM7fVPvE&si=j_kM_rOyjrpNhYyA) where the hosts discussing software gaps in the Rust ecosystem for supporting software traceability and document maintenance. This is one person's attempt at a solution, and one person can't know whether it's the right one. The limitations above are the ones I can see; the ones I can't see are the reason this needs other contributors, especially maintainers of open source projects and developers with backgrounds in safety-critical development.

If you work in a domain where traceability matters — safety-critical, regulated, or just a team that's been burned by stale docs — your perspective on what Rubric should actually do is more valuable than code. Issues, design feedback, and use-case reports are all welcome. And so are pull requests, of course :)

## License

Dual-licensed under either of:

- Apache License, Version 2.0 (LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license (LICENSE-MIT or http://opensource.org/licenses/MIT)

at your option.
