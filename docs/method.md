---
name: method
description: >
  How defects and regressions actually get found, how a hypothesis gets
  verified before it is acted on, and how a codebase is stopped from drifting
  back. Use when investigating a bug, chasing a performance problem, deciding
  what to automate, or setting up the checks that hold a decision in place.
---

# Finding, verifying, and not drifting

The patterns in the other documents are the *conclusions* long-lived systems
reached. This one is about the machinery that produced them, which is the more
valuable half: a team that can reliably find, prove, and hold will rediscover
every good pattern on its own. A team that cannot will lose them one refactor at
a time.

Three phases, and most codebases are weak at a different one than they think.

1. **Finding** — most defects are not found by reading code. They are found by
   an instrument that was built before anyone knew what it would catch.
2. **Verifying** — a hypothesis is not a finding. The gap between "this looks
   wrong" and "here is the mechanism, the preconditions, and the blast radius"
   is where most wasted work happens.
3. **Not drifting** — a fix with no artefact holding it in place has a half-life
   of about two refactors.

---

## 1. Finding

Ranked by what each instrument catches that nothing else can. Build them in this
order.

### Properties and fuzzing

The highest-yield instrument for anything that parses, serialises, or transforms.
You do not assert an output; you assert a **property that must hold for all
inputs**, and let a generator attack it.

The properties that pay:

- **Round-trip** — parse, print, parse again; the two structures must be equal.
  One target of this kind finds an enormous class of parser and printer bugs
  that no example-based test would reach.
- **Differential** — two implementations of the same contract must agree. A fast
  path and a reference path, an optimised strategy and the naive one.
- **Invariant** — whatever the module's stated guarantee is, asserted after every
  operation.
- **Never panics / never hangs** on arbitrary input, for anything reading data
  you do not control.

Three details separate a fuzz target that earns its keep from one that does not:

- **Scope the property to where it actually holds.** A good round-trip target
  explicitly *rejects* the generated inputs where the property is legitimately
  ambiguous, rather than weakening the assertion for everyone.
- **Make the crash output directly actionable.** Customise how the generated
  input prints so the failure report contains the reproducible form — the
  stringified pattern, the serialised payload — not just an opaque structure
  dump.
- **Check every finding in as a permanent test**, in a directory of nothing but
  fuzz-derived regressions.

### Exhaustive checking for concurrency

For anything with more than one thread of control, ordinary tests sample the
interleaving space essentially at random and will not find the bug. A model
checker that enumerates the permutations will.

The practical shape: route every concurrency primitive through one internal
module that re-exports either the real implementations or instrumented ones
under a build flag, so the whole program becomes checkable by changing a build
configuration. Shrink capacity constants under that flag so exploration
terminates.

Cost is the reason people skip this, and there is a good answer to cost: **run
the expensive check on every merge to the main branch, and on a pull request
only when it carries a label saying it touches that area.** Contributors add the
label; the maintainer adds it when they notice. And run it as a matrix over the
alternative implementations, because "it works with the other queue" is exactly
the kind of thing nobody checks by hand.

### Checkers that need no test authoring

Undefined-behaviour interpretation, address and memory sanitisers, leak
detection, a dependency-vulnerability audit on a nightly schedule. These find
things nobody wrote a test for, which is their entire value. Each is a separate,
narrowly named job — a single "test" job that does everything tells you only
that *something* broke.

### One corpus, many implementations

When you have several implementations of one contract — a fast path and a
reference, several engines behind a selector, several platform backends — do not
write a test suite per implementation. Write the cases **as data** in a
declarative format and run the whole corpus against every implementation.

The stated reasoning, from a codebase that migrated to this after the number of
implementations grew: the per-implementation approach was slower to compile,
harder to maintain, and made the test definitions themselves less clear. Tests
as data are also readable by someone who does not know the codebase, which
matters more than it sounds.

### Building the whole matrix, not just the default

Compile-only jobs are cheap and catch a category nothing else does: a feature
combination that does not build, a minimum toolchain version that broke, a
minimal-dependency-version resolution that fails, a platform that no longer
compiles. Run the feature powerset. It is the only way to know that your
optional capabilities are actually optional.

### Your consumers' test suites

The strongest single practice observed anywhere: **CI jobs that check out major
downstream projects and run *their* tests against the current branch.** A
breaking change is then caught by the people it would break, before merge,
automatically. Nothing else gives you that signal, and no amount of internal
testing substitutes for it.

The version you can afford, if you have no external consumers: run the app's own
end-to-end suite against the library change, in the same pipeline.

### Measurement, kept as a record

- **Profile before believing anything about performance.** The function you
  suspect is usually not the one.
- **Read the timing data you already emit.** One team found a badly balanced
  build pipeline by parsing the timestamps out of their own CI logs — no new
  instrumentation, just looking at what was already there.
- **Keep the results in the repository**, dated and labelled with the machine
  and configuration they were taken on. Directories of dated benchmark runs, one
  per environment — including variations like allocator and libc, because those
  move results more than most code changes do. One such record is explicit that
  there is no tooling to compare them and it is done by hand; it is *still*
  worth having, because the alternative is a number in a pull request comment
  from two years ago that nobody can reproduce.
- **Measure the costs that are not runtime.** Compile time from scratch, and
  binary size per feature, computed against a hello-world baseline. A feature
  flag that adds nothing to runtime and two seconds to every build is still
  expensive.

### Production, and the users in it

The workloads your benchmarks model are the ones you thought of. The regression
that matters will be in a shape you did not imagine, and the only instrument
that sees it is telemetry you built before you needed it.

Build the counters and timings *now*, behind a flag, on the paths you believe are
hot. The reason is concrete: in the best-documented regression in this research,
a user diagnosed an 8.5% CPU increase themselves using per-worker runtime metrics
and handed over a twenty-line reproduction. Without those metrics the report
would have been "it got slower", which is unactionable.

### Coverage, as a finder and never as a target

Measuring coverage to find the code nobody has ever executed is valuable —
writing tests for one such area surfaced three probable bugs. Measuring coverage
as a number to raise produces tests that assert nothing. The tell is whether the
result is *"we found these three things"* or *"we got to 80%"*.

### Bots, LLMs, and other hypothesis generators

Automated reviewers, static analysers, and LLM audits are good at producing
*candidates*. They are not evidence. Observed, and worth copying exactly:

- A subtle algorithm bug was found by asking a model to look for bugs in one
  specific function — and the pull request says so plainly.
- A bot flagged a retry-ordering flaw; the author said it was fixed; **a human
  reviewer re-derived the failure path, found it was not fixed, and said so.**
  That reviewer is where the value was, not the bot.
- One project's contribution policy requires that a human be able to explain the
  change in their own words, and that AI-derived context be quoted and marked as
  such rather than pasted as if it were the contributor's reasoning.

Treat every generated finding as a hypothesis that enters phase 2 below. Never
merge one on the strength of its confidence.

---

## 2. Verifying

**Reproduce before fixing.** A fix for a defect you cannot reproduce is a guess
with a diff attached, and you will not know whether it worked.

**Minimise the reproduction, and treat it as the deliverable.** Twenty lines
that fail deterministically are worth more than the original report, more than
the fix, and often more than the analysis — because they outlive all three as a
test.

**Explain the mechanism, not the symptom.** The best defect write-ups seen have
a specific shape:

- The exact chain of events, traced step by step.
- **A numbered list of the preconditions required to trigger it** — one such
  write-up lists four, including the non-obvious one about ordering.
- **An explicit bound on the blast radius**: "because of precondition three, this
  can never produce a false positive or a false negative; it is strictly about
  which span is reported." That sentence is what lets everyone else decide
  whether they are affected.
- Where a regression: the commit that introduced it, by hash and subject.

**Do not re-run a flake.** When a test fails intermittently, the discipline is to
reconstruct the interleaving that produces the observed value and explain why it
is now possible. Observed in practice: a maintainer explained, step by step, why
a scheduling change legitimately removed one wake-up and therefore changed a
counter — no re-runs, no "probably flaky".

**A stated claim is a hypothesis.** The highest-value review comment in every
codebase examined is some form of *"I do not think that is actually fixed"*,
followed by the reviewer walking the path themselves. Apply it to your own work
first: take your own claim that a case is handled, and try to construct the
interruption, repetition, or re-entry that breaks it.

**Try to refute, not to confirm.** For anything subtle, the useful question is
not "does my test pass" but "what input would make this wrong". Confirmation is
cheap and nearly worthless.

**For a pure performance change, equality of output is the correctness
argument.** Not "the tests pass" — byte-identical results across the full fixture
corpus *and* on pathological inputs. If the change is only supposed to alter
timing, that is provable, so prove it.

**Measure the fix, and report honestly when it does not help.** The two moves
that mark a serious engineer, both observed verbatim:

- Trying the obvious narrow fix, discovering it **reintroduces the exact defect
  the original change existed to prevent**, and saying so publicly instead of
  shipping it.
- Trying a cleverer fix, measuring it, and reporting "this does not help as much
  as I hoped" rather than quietly merging it.

**Ask what the fix costs, not just what it buys.** A correctness improvement that
loses 8% CPU on a real workload is a trade, not a win, and it needs to be named
as one.

**Reverting is a normal outcome.** When a change costs more than it bought, take
it out rather than carrying it plus a partial fix while hunting for the real one.
And an optimisation nobody can safely modify should be removed whatever its
benchmark says — one maintainer reverted a subtle optimisation entirely with the
reason "this is too subtle and I do not have time to fix it properly", which is
the correct call and a rare thing to say out loud.

---

## 3. Not drifting

A finding with no artefact behind it will be reintroduced. Every phase-2
conclusion should leave something that fails when it stops being true.

### Turn the finding into an artefact

| What you learned | What holds it |
| --- | --- |
| A specific defect | A regression test named after the defect, failing on the old code |
| A defect found by fuzzing | The input checked into a fuzz-regression directory, **with a comment explaining the mechanism** |
| A subtle semantic | Documentation *plus* tests asserting exactly what the documentation claims |
| A type-level invariant | A compile-fail test with a snapshot of the error message |
| A performance property | The relevant counters written into the recorded test baseline |
| A structural rule | A test over the module graph that fails on violation |
| An interface promise | A compatibility check, and a list of which external types may appear in the public surface |
| A decision that was expensive to make | A dated decision record, including the options rejected |

**Record the mechanism in the regression test, not just the input.** The best
fuzz-regression files carry paragraphs explaining what the engine was doing, why
the bug was possible, and — this is the valuable part — **the more thorough fix
that was considered and not done, and why.** Without that, the next person
"fixes" it properly and reintroduces something worse.

**Name the change that introduced the defect** in every regression fix. One line,
one blame, and the history becomes queryable: what did this break, how long did
it take to notice, which areas keep regressing.

### Make drift impossible rather than discouraged

- **One place for policy.** Conditional-compilation and feature gating live in
  one file as named capability macros; call sites reference the capability, never
  the raw condition. The answer to "what does this feature actually enable" is
  then a file you read, not a search.
- **Write complementary conditions so a human can verify them by eye.** An
  observed review demand, almost verbatim: *these are out of sync; one should be
  written as exactly `not(...)` around what the other writes — do not apply
  de-Morgan transformations.* Two conditions requiring boolean algebra to check
  are two conditions that will drift.
- **Documentation and enforcement must be provably in sync.** Another observed
  review: the docs claimed three things were unsupported, but only two had a
  guard. The demand was not "fix the docs" but "make them match and keep them
  matching".
- **New surface enters behind an instability gate**, and the promise is made
  later as its own deliberate decision. Otherwise every merged feature is an
  accidental permanent commitment.
- **Update the prose the change invalidates, in the same change** — and grep for
  the term, because the invalidated sentence is rarely in the file you edited.
- **Log a signal that tells you when a heuristic has gone stale.** A pipeline
  that shards work by measured timings also logs the diff against the previous
  measurement, specifically so the team notices if run-to-run variance grows
  large enough that the approach has stopped working. Build the thing that tells
  you your optimisation is no longer true.
- **An escape hatch is a symptom, tracked as debt.** When the answer to a design
  flaw is "there is a flag for that", the flaw is still there and has grown a
  configuration surface — and the flag only helps people who already know they
  need it, which is nobody until they have been hurt.

### Cadence — what runs when

Everything above costs something. The way to afford it:

| Frequency | What |
| --- | --- |
| Every commit | Fast unit and integration tests, lint, format, type check |
| Every pull request | The full ordinary suite, the compile matrix, interface-compatibility checks, and the expensive area-specific checks *when the change carries the label for that area* |
| Every merge to main | Everything, unconditionally — including the exhaustive concurrency checks that are label-gated on pull requests |
| Nightly | Dependency audit, long fuzz runs, stress tests, platform matrices |
| Per release | Downstream consumers' suites, benchmark run recorded into the repository with its environment |

The label-gating idea is worth stealing on its own: it lets an expensive check be
opt-in for contributors and mandatory at the point where mistakes become
permanent.

---

## For this repository, now

In rough order of value per hour, given where the code currently is:

1. **A minimal reproduction, before any fix.** Free, and it becomes the test.
2. **Regression test per defect, named after it.** Free.
3. **Injected clock and injected host seam**, so timing and window behaviour are
   testable at all. The host seam already exists — keep everything going through
   it.
4. **Property tests on the pure geometry**: panel sizing and edge anchoring, for
   arbitrary screen rectangles and panel dimensions. The invariant is easy to
   state — the frame is on-screen, within the work area, and on the intended edge
   — and this is exactly the shape of code where a generator finds the case you
   did not consider.
5. **Round-trip property on settings**: any settings value serialises and
   reloads equal, and any file with keys missing loads with defaults. That is the
   invariant the current design already relies on; nothing currently asserts it.
6. **A compile matrix**: build the frontend with and without the desktop host,
   because the browser-only path is a real supported mode that nothing checks.
7. **Structure test for the absences** once there is more than one module.
8. **Dated benchmark records in the repo** once there is anything worth timing —
   startup to first paint, summon latency — with the machine written down.

Do not build a fuzzing pipeline or a model checker for an application this size.
Build the seams that would let you, and add the instrument when a class of defect
starts recurring.
