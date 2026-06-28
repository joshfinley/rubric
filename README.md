# rubric

Rubric is *coherence infrastructure* for software development in Rust.

## Status

Rubric follows [Semantic Versioning](https://semver.org/). The current version is v0.2.0 pre-release.
While Rubric's public interfaces and behavior are subject to change, any and all feedback (via [GitHub Issues](https://github.com/joshfinley/rubric/issues)) from
real-world project use on the v0.2.0 tag would be instrumental for progress to a v1.0.0 release,
and would be greatly appreciated.

## Motivation

Rubric aims to provide coherence as a first-class engineering property for
software projects that use both Rust and Git. The project idea was spawned
from a [Self Directed Engineering podcast episode on traceability](https://music.youtube.com/watch?v=-f6RM7fVPvE&si=j_kM_rOyjrpNhYyA) and has since been expanded to integrate
concepts from [aspect-oriented programming](https://en.wikipedia.org/wiki/Aspect-oriented_programming) (AOP), version control, and
software [requirements traceability](https://en.wikipedia.org/wiki/Requirements_traceability).

Rubric's motivation stems from the Rust ethos of zero-cost abstractions,
manifesting as "eating your cake and having it too" in ways that are
not easily accessible in other language ecosystems. It synthesizes
concepts from aspect-oriented programming and traceability which have
floundered in practical use. The project of AOP is to identify and
separate *cross-cutting* concerns in software and inject desired
behavior without modifying code. This has manifested in AOP projects
as a kind of quiet "action at a distance", which presents as the inverse
of requirements traceability: the intent to provide explicit mapping
of relationships between software components and requirements that software
exists to satisfy.

Rubric provides a synthetic view of these apparently opposing concerns:
AOP and requirements traceability may be viewed as concerns for software
components and the overall software development lifecycle applied from
different ends. Many classes of requirements exist as cross-cutting concerns,
which is the exact thing which AOP seeks to separate out. Both relate
to the correspondence between _intent_ and _implementation_. Rubric views
these relations as the same _type of thing_ which AOP and requirements
traceability operate on. Where AOP
seeks to separate the two, often making intent invisible by injecting it,
requirements traceability frameworks reach to consolidate. If the relations
are the same or similar "type of thing" in both views, then AOP can be
read as forward application of the correspondence between intent and
implementation, and requirements traceability as the backward view.

While these two paradigms have been wilting under the ecosystems
they primarily exist in, there have been important developments in others.
Git is one example. Git arose after [BitKeeper booted users from its
free version in 2005](https://lwn.net/Articles/130746/). This was around
the time of the peak and decline of AOP. Meanwhile, requirements traceability
has been working quietly in the background for safety-critical software
the entire time, a discipline that is intentionally oblivious to new
and shiny things. Despite that obliviousness, Rust continues to make
inroads in safety-critical software.

Rubric exists to unify the four (AOP concepts, requirements traceability,
Git, and Rust) into *coherence infrastructure* for projects written in Rust.

## The Model

Rubric minimizes the tracking of software requirements into one thing:
_an attribution chain._ This is the traceability import where cross-cutting
concerns can be manifested in an observable way in a source tree. It is a
triplet of

> (requirement, satisfier, verifier)

A requirement is a labeled, version controlled statement in `rubric.toml`.
The satisfier is some source code that realizes a requirement and the
verifier is something that demonstrates it. The legs of this triplet
are **sealed** (content-hashed) and version controlled as well. Sealing
exists to make changes to any of the three legs visible. Changes such
as these will be alerted on when `cargo rubric check` is invoked until
the changes are accepted.

Requirements may sometimes manifest as _cross-cutting_ concerns, where
they may be scattered across different functions and tests. These associations
and changes which may impact them are very difficult to keep track of
by hand. Without automation, that tracking becomes a burden on developers,
slowing development and hollowing out the value to be accessed by efficient
traceability. However, automated binding of the elements of a requirement
allows for those bindings to be _enumerated_ one annotation at a time or
_quantified_ by a [pointcut](https://en.wikipedia.org/wiki/Pointcut)
that selects a set of items at once. In either case, the binding is
sealed and becomes an obligation which `cargo rubric check` enforces.

## Quick Start

Rubric is available through Cargo and [crates.io](https://crates.io/crates/cargo-rubric).

```bash
cargo install cargo-rubric
cd path/to/your/project
cargo rubric init
```

Requirements are defined in `rubric.toml`:

```toml
[req.VOTER-1]
kind = "functional"
statement = "Two identical inputs always out-vote the third"
```

Then, annotate the code that satisfies or verifies the requirement:

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

Once satisfied with this, accept and verify:

```bash
cargo rubric accept     # records the chain, seals all three legs into rubric.lock
cargo rubric check      # verify seals (run locally or in CI)
```

From now on, editing `vote`'s body or test, or rewording the statement, makes
`check` fail and name the failure:

```
✗ VOTER-1 "Two identical inputs always out-vote the third"
    crate::vote changed since last accept
```

Reformats which don't change lexical tokens leave the seal intact.

Broken seals may present as noise during development if used improperly.
However, careful and thoughtful use of Rubric translates changes
into a drift signal with context, which can then be used to make informed
review and editing decisions. Once this is done, running `cargo rubric accept`
re-seals and the resulting `rubric.lock` diff can be carried with the commit /
pull request, providing an additional _temporal_ signal on top of the mechanical
one. Further review and editing can take place here, providing a convenient,
automated substrate for development norms to build around.

## Examples

This repo includes a living example in `examples/tmr-voter` to demonstrate a
simplified Rubric use case. [Quick Start](#quick-start) binds one requirement
to one function by hand, but requirements can also be scattered across
multiple items at once. In `examples/tmr-voter`, three diverse channels feed
a majority vote and the notion of "every channel is reviewed" is a concern which
doesn't live in any single function:

```
<SNIP>
[req.TMR-CHANNELS]
kind = "invariant"
statement = "Each redundant channel is an independently reviewed public entry point"
seal = "full"
cover = "pub fn within crate::channels"        # the pointcut binds every channel
reconcile = true
```

`TMR-CHANNELS` is the cross-cutting concern while `cover = "pub fn within crate::channels"`
is a _pointcut_ that binds the three `pub fn` channels with no per-channel annotation.
Adding a fourth channel results in `cargo rubric check` catching the exceptional change:


```
$ cargo rubric check
✗ TMR-CHANNELS "Each redundant channel is an independently reviewed public entry point"
    pub item not covered yet; run accept to acknowledge — crate::channels::channel_d

$ cargo rubric accept
+ TMR-CHANNELS crate::channels::channel_d

$ cargo rubric check
✗ TMR-CHANNELS "..."
    re-sealed but not attested; run `cargo rubric attest` after review

$ cargo rubric attest
attested TMR-CHANNELS: attest:7f3d12821cba7a6c

$ cargo rubric check
✓ rubric: chain intact (2 requirements)
```

Here, a new `pub` fails the `check` until someone acknowledges it with `accept`. In a more
realistic situation, a separate someone could then vouch for the reviewed surface with
`attest`. `vote` itself is sealed by signature and body and demonstrates binding
to external failure-mode evidence, which is also content-sealed.

## Pointcuts for Coverage Quantification

Rubric also supplies a designator called `cover` to select a set of items by
visibility, kind, and module scope:

```
cover = "pub fn within crate::api"      # every public function under crate::api
cover = "pub within crate::audited"     # every public item under crate::audited
cover = "any item within crate::ffi"    # every item, regardless of visibility, under crate::ffi
```

Any item matching the cover is bound as a satisfier of the requirement and sealed. This
provides tracing for items in audited modules without mandating a manual annotation for
each, and is a good example of how AOP and requirements traceability agree on operating on
relations but from opposite ends. As a project evolves, a new `pub` would not silently
escape the rule because `check` reports such items as uncovered until accepted while
signature seals catch private items that are turned public later on.

## The Seal

Rubric seals are scheme-tagged content hashes (FNV-1a 64) of normalized input with one chain
per leg of the requirement triplet:

- `stmt:` seals the requirement's statement text
- `body:` seals a satisfier or verifier body minus whitespace or comments

What each cited item hashes may be manipulated by `seal`ing mode:

- `body` (default): the item's block body.
- `signature`: the item's signature, including visibility (e.g., `fn` vs `pub fn`)
- `full`: signature and body together
- `off`: existence only, no content hash

These modes describe source items. External evidence (an `external:` path) is always sealed by
its file bytes under the `file:` scheme, whatever mode the requirement names. A requirement may
mix the two, as the example does. One whose every leg is external but which still asks for
`signature` or `full` has no source to seal, so `check` reports the mismatch.

It's important to note that token hashes intentionally break on refactors that don't
ostensibly change behavior (e.g., renaming a local or reordering independent statements).
This was chosen because discipline for such changes is a project _policy_ concern which
Rubric should not paper over - it's up to any given project's maintainers to set
standards and norms to decide how to deal with this, though rubric honors the issues
that such sensitivity can present by providing the "`off`" sealing mode.

## Attestation and Reconciliation

Reconciliation is opt-in per requirement with `reconcile = true`. While `accept` re-seals
chains from current source, `attest` provides a second, deliberate step for recording a
requirement's _attestation root_, which is also itself a hash over its current leg seals.
This is recorded under the reserved `<attest>` entry in `rubric.lock`.

Because `accept` never writes `<attest>`, re-`accept`s which move an attested leg leave the
recorded attestation root stale. `check` will then report this as an unreconciled item until
`attest` is run again, ideally after a proper review.

This mechanism decouples sealing (what the code is) from attestation (whether someone vouched
for it). A haphazard `accept` re-seals the legs but does not automatically produce a fresh
attestation root.

You might notice that Rubric might be abused by a determinedly ignorant or malicious contributor by carelessly
re-`accept`ing and re-`attest`ing changes. Rubric draws another responsibility boundary here - in order
to best leverage information that is available within a project's working interior, Rubric intentionally
hands off guards against history rewrites and careless use to external guardrails. This decision
enables Rubric to operate on another axis: time (or change) via Git.

## Git as a Temporal Ledger

`cargo rubric log` walks the git history of `rubric.lock` and renders a timeline of seal events.

For each `(requirement, item)` pair, the timeline records the hash transition, the commit, and the author, with the underlying source delta one `git show` away.
By deferring the history to Git, Rubric provides convergence points across time for audit surfacing. `cargo rubric audit` turns that history into a check by walking the commits of `rubric.lock` and flagging commits
where a `reconcile` requirement's leg seal moved without its attestation root also moving in the same commit.

For example: an ignorant (but not subversive) contributor that ships a reckless `accept` leaves evidence:

```
$ cargo rubric audit
commit 817cd6275  2026-06-28  Tester
    API crate::api::connect   re-sealed without attestation
```

Meaning that a project with healthy CI policies may track such events and respond in turn. By stepping out
of the way from CI tools specifically designed to audit history integrity, Rubric leaves harsher but
more effective integrity protection techniques open to this layer. Delayed repository mirrors can still
use Rubric's CLI if the project has a demonstrated history of Rubric hygiene. Meanwhile, ordinary origin branch protection may be used to defend against more common cases involving careless history rewrites.

## Annotation Forms

The earlier satisfier and verifier examples use the simplest form of annotation which Rubric supports - comments. However, Rubric also offers two other flavors of annotation to suit the taste of a given project:

| Form | Spelling | Requires |
|---|---|---|
| Comment | `// satisfies: VOTER-1` | Nothing |
| `cfg_attr` | `#[cfg_attr(any(), satisfies(VOTER-1))]` | Nothing (compiler-inert) |
| Bare attribute | `#[satisfies(VOTER-1)]` | `rubric-trace-macros` (identity pass-through, ~15 lines) |

`cfg_attr(any(), ...)`: `any()` with no arguments is always false, so the compiler discards the attribute without resolving `satisfies`. The `cfg_attr` and comment annotation forms might be preferable for projects which wish to limit or exclude `proc-macro`, external dependencies, and hypothetical runtime effect.

The same applies to `verifies:` / `#[verifies(...)]` on tests. For items no annotation can reach, declare the path directly in `rubric.toml`, or select a set of them with a `cover` pointcut.

Labels are bare requirement IDs (`VOTER-1`, `PARSER-3`), similar to conventions which many requirements documents already use. There's no module-path namespace and no generated `reqs` module. A typo in a label is caught by `check` as an orphan annotation.

The power here is that these markers live in source text and are present for all builds operating on that source. Nothing is injected at compile time or needs compiling out, which is where Rubric intentionally diverges most from historical examples of AOP-paradigm tooling.

## Contributing

Rubric started from [a podcast episode on traceability](https://music.youtube.com/watch?v=-f6RM7fVPvE&si=j_kM_rOyjrpNhYyA) discussing gaps in the Rust ecosystem for software traceability and document maintenance. The project is early-stage and all contributions are welcome.

## License

Dual-licensed under either of:

- Apache License, Version 2.0 (LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license (LICENSE-MIT or http://opensource.org/licenses/MIT)

at your option.
