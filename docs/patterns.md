# Patterns worth stealing

Concrete techniques observed in high-performance, long-lived systems code —
runtimes, HTTP stacks, search engines, concurrency libraries. These are the
implementation-level moves behind the principles in
[system-architect.md](system-architect.md). Each one is cheap to adopt and each
one has a clear reason to exist.

---

## Structure

**The dependency graph is a strict DAG of libraries with the product at the
top.** The most maintainable large codebases are not one program; they are a set
of genuinely independent libraries plus a thin binary that composes them. The
binary depends on everything. *Nothing depends on the binary.* If you cannot
draw that picture for your system, the product has leaked into the libraries.

**The abstraction crate depends on nothing.** In the strongest examples there is
a small module in the middle that defines only the trait — the port — and it has
zero internal dependencies. Implementations depend on it. Consumers depend on
it. Neither knows about the other. This is what makes two competing
implementations pluggable without either being aware it has a rival.

**Runtime-agnostic by defining ports, not by abstracting late.** A well-built
library defines the traits it needs from the outside world — how to spawn, how to
sleep, how to read and write — and ships the adapters for a specific environment
in a *separate* package. The core then has no opinion about its host, and the
opinion lives in one place where it can be swapped.

**One canonical extension mechanism per system.** When a type must support many
underlying representations, the good implementations pick a single mechanism —
one function table, one trait — and route everything through it. Every
representation is then just data plus a table; adding one is additive and
touches nothing existing.

**A small core that almost never breaks, orbited by packages that churn.** In a
mature family of packages you can read the discipline straight off the version
numbers: the core sits on a long-lived minor version while the satellites have
had several breaking releases. That gap is the design working. The core holds
only what everything must agree on — identity, the central trait, the dispatch
point — and everything opinionated lives outside it, free to break.

---

## Seams

**A shim module that swaps implementations under test.** Rather than importing
concurrency primitives from the standard library, high-assurance code imports
them from an internal module that re-exports either the real ones or
instrumented ones, chosen by a build flag. Every file in the codebase imports
from the shim. One switch turns the entire program into something a model
checker can explore exhaustively.

The transferable form is more general than concurrency: **route every dependency
on the unpredictable through one internal module, so a single flag replaces all
of it at once.** Time, randomness, the filesystem, the network, the OS. The cost
is a one-line import change; the payoff is that determinism becomes a build
configuration rather than a refactor.

**Shrink the constants under test.** The same code that uses a 256-slot queue in
production uses 4 under the model checker, because exhaustive exploration of 256
slots does not finish. Sizes that exist to make exhaustive testing tractable are
worth the conditional.

**Compile-fail tests.** Some of the most valuable tests assert that incorrect
usage *does not compile*, with a snapshot of the exact error message. This is
how you protect an invariant you have encoded in the type system: without the
test, a later refactor can quietly make the illegal state legal again and
nothing goes red.

---

## Feature and capability gating

**Never write a raw conditional-compilation attribute at a call site.** The
disciplined codebases define a named macro per capability in *one* file — dozens
of them — and every call site uses the named macro. The condition itself lives in
exactly one place, so the answer to "what does this feature actually enable" is a
file you can read rather than a search across the tree.

This is mechanism-vs-policy at the build level: the call sites say *what
capability this needs*; the one file says *what that capability means*.

**Complementary conditions must be syntactically recognisable as complements.**
An observed review comment, almost verbatim: *these are out of sync — one of
them should be written as exactly `not(...)` around what the other writes; do
not perform de-Morgan transformations.* Two conditions that a reader must do
boolean algebra to check are two conditions that will drift. Write them so
correctness is verifiable by eye.

**Gates and documentation must be provably in sync.** Another observed review:
the docs claimed three features were unsupported on a platform, but only two had
a compile error guarding them. The demand was not "fix the docs" — it was *make
them match, and keep them matching*. Documentation that can drift from
enforcement is documentation that is eventually a lie.

**New public surface enters behind an instability gate.** The convention that
makes long-term API stability affordable: anything not yet ready to be promised
forever ships behind an explicit unstable flag, and the promise is made later,
deliberately, as its own decision. Without this, every merged feature is an
accidental permanent commitment.

---

## Concurrency and failure

**The module doc *is* the safety protocol.** The best concurrent code opens with
a long comment that enumerates: every kind of reference to the object, what the
state bits mean, and — field by field — *who may access this field, when, and
under what condition*. Rules are numbered so review comments can cite them. This
is not decoration; it is the only place the invariant exists, because the
compiler cannot express it.

Adopt the format even for modest state: **for each field, one line saying who
writes it and who reads it.**

**Encode the concurrency contract in type names.** A single shared buffer exposed
as two types — one meaning "producer handle, single thread only", one meaning
"consumer handle, any thread" — makes the rule impossible to violate by
accident, and puts it in the reader's face at every use site.

**Fallible operations are marked, tested, and documented as a set.** Where a
function can abort, the convention is: annotate it so the failure is attributed
to the caller, document the exact condition, and add a test *in the file that
collects those tests* asserting it fails where it should. Failure behaviour is
treated as public API with its own test suite, not as an accident.

**Cite the issue that caused the design.** Comments in these codebases regularly
say "wider integers here to mitigate a wraparound race — see issue NNNN". The
history of *why the obvious version was wrong* is the single most valuable thing
to leave behind, and the cheapest place to leave it is next to the code.

**Fix it upstream.** An observed review, in full: *did you open a pull request
against the dependency?* Working around a bug in a layer you depend on is a
permanent local cost to avoid a one-time external one. The reviewer's instinct
was to push the fix to where the defect actually is.

---

## Tests

**Integration tests go through the public door, always.** Observed: a
contributor's test was rejected from beside the code and had to be rewritten as
an integration test — which forced them to reach the behaviour through the public
API, because the internal type is not visible from outside. The inconvenience
*is* the value: it proves the behaviour is reachable the way a user would reach
it.

**One test file per concern, named for it.** Flat directories of many small
files named `<area>_<behaviour>.rs`, not a few large ones. Regression tests are
named after the defect — a file called `<area>_memory_leak` or
`<area>_fd_leak` tells you what it protects without being opened.

**Separate test trees for separate questions.** Distinct top-level directories
for: does it compile under every feature combination; does it work with real
external components; does it survive sustained load; how fast is it. Each answers
a different question, runs on a different cadence, and fails for a different
reason. Merging them means the slow one stops being run.

**Run your consumers' test suites in your own CI.** The most striking single
practice observed: the CI pipeline includes jobs that check out major *downstream*
projects and run their tests against the current branch. Breaking changes are
caught by the people they would break, before merge, automatically. This is a
contract test with the ecosystem, and it is the strongest form of "stable
boundary" enforcement there is.

**Many narrow CI jobs, not one big one.** Forty-plus separately named jobs, each
answering one question: minimum supported version, minimal dependency versions,
the full feature powerset, undefined-behaviour checking, address sanitiser,
memory checking, semver compliance, which external types leak into the public
API, docs build, spelling, per-platform builds. A single "test" job that does
everything tells you only that something broke.

**Delete tests that do not earn their place.** Observed review: *I'm not sure
this test provides much, do we need it?* — and the contributor's honest answer
was that it existed for coverage. Coverage is not a reason.

---

## How changes to the core get made

Everything above is about steady state. This is the part that decides whether a
system stays fast and correct while it keeps changing — drawn from reading the
actual history of scheduler, task, and synchronisation-primitive changes rather
than surface-level ones.

**A big feature lands as inert infrastructure first.** The pattern for a large
subsystem: the first change adds the types, the driver hooks, and the gating —
several hundred lines — and *does nothing*. It is entirely behind an instability
flag, and the description says plainly that the actual operations come in later
changes and that this one cannot affect existing behaviour. Then one operation
per change after that. The alternative — one enormous change that both builds the
machinery and uses it — cannot be reviewed, and cannot be reverted without
losing everything.

**Decompose the change and map the steps to commits.** The best change
descriptions have two headings: *Motivation* and *Solution*, where motivation
links every prior report and failed workaround, and solution is a numbered list
of steps, each naming the individual commit that performs it. A reviewer can
then review one idea at a time. This is a five-minute authoring cost that buys a
qualitatively different review.

**Say what you deliberately did not do, and why.** From one such description,
paraphrased: *I chose not to move this logic into the queue, because it depends
on other worker state; instead the queue exposes two small operations and the
run loop stays a small diff.* Recording the rejected restructuring is what stops
the next person from "tidying" it into the version that was already considered
and rejected.

**Restructure so the guarantee becomes provable.** One change to a work queue
altered which half of the queue overflows — not to fix an observed bug, but so
that a documented fairness property could be *argued* rather than hoped for. The
description works through the proof: every operation moves an item strictly
closer to being run, therefore no item can starve. **A documented guarantee that
cannot be argued from the code is a guarantee that will quietly stop being
true.**

**Document the guarantee the implementation already provides.** A separate
change added only documentation: the implementation had careful ordering
guarantees throughout, and none of it was visible to users. An internal property
nobody has stated is one a future refactor will discard without noticing.

**Pin semantics with tests, not just prose.** A change that clarified a subtle
edge case in a channel's overflow behaviour was three hundred lines of docs *and
tests asserting exactly what the docs now claim*, with no behaviour change at
all. This is the answer to documentation drift: make the doc an assertion that
fails.

**Weigh the fix against the interface.** From a bug fix, verbatim in substance:
*the alternative I considered was returning an error, but that is an interface
change, which I would like to avoid.* Choosing the smaller fix because the
larger one breaks a promise — and saying so — is the routine case, not the
heroic one.

**A dependency is a policy decision with a memory.** One change was reverted
with the reason: *do not add new dependencies without very strong justification;
this particular one we removed previously when it raised its minimum compiler
version and broke ours.* The institutional memory of a specific past injury,
applied as a rule. It was then re-landed using the in-house equivalent, and the
re-landed version was smaller than the original.

**Revert rather than patch a regression.** The most instructive sequence in the
whole history:

1. A long-standing latency pathology had an escape hatch — an unstable option to
   turn the optimisation off. A maintainer argued the escape hatch was the wrong
   answer, because users cannot know in advance that they need it: you write
   ordinary code, hit an unexplained stall in production, and *then* discover the
   flag exists.
2. A structural fix landed instead: ~350 lines across ten files, making the
   contended slot participate in work-stealing like everything else.
3. A reviewer asked whether any performance regression test covered the changed
   hot path. Benchmarks were run and reported in the thread.
4. A test flaked on one platform. The maintainer did not re-run it — they
   reconstructed the exact interleaving that produced the new value and explained
   why the metric legitimately changed.
5. Docs were required to be updated, with an instruction to grep the whole
   codebase for the term because prose elsewhere had been invalidated.
6. It shipped. A user then reported **8.5% higher CPU in production** on a
   workload the benchmarks did not model, with per-worker runtime metrics
   showing the cause, and reduced it to a twenty-line reproduction.
7. The author tried the obvious narrow fix, discovered it reintroduced the exact
   deadlock the change existed to prevent, and *said so publicly instead of
   shipping it*. A second, cleverer attempt was measured and honestly reported as
   not helping much.
8. The whole thing was reverted — a change of +56/−349 undoing the original.

Four things to take from that. **Benchmarks model the workloads you thought of**;
production finds the others. **Observability is what makes a regression
diagnosable** — the metrics that let a stranger find the cause were built in
deliberately, behind a flag, before anyone needed them. **A fix that reintroduces
the original defect is not a fix**, and the discipline is to say so out loud.
And **reverting is a normal move, not a failure** — cheaper than carrying a
partial fix and a known regression while hunting for the real one.

**An escape hatch is a symptom.** The recurring judgement: when the answer to a
design flaw is "there's a flag for that", the flaw is still there and now has a
configuration surface attached to it. Flags added to work around a design are
tracked as debt to be removed by fixing the design.

---

## Review moves seen repeatedly

These are the actual comments that recur. They make a good self-review checklist,
because each one is a mistake competent contributors make constantly.

- *"This name is now wrong."* — the change was correct; the name no longer
  describes it. Names are corrected in the same change that invalidates them.
- *"I don't think this field is used for anything?"* — state added to serve a
  speculative accessor gets deleted, along with the accessor, along with the
  code that maintained it.
- *"An extra lock just to synchronise this feels heavy-handed; I'd rather a
  solution that doesn't need one."* — reaching for a synchronisation primitive
  is treated as a design smell to be argued out of, not a default.
- *"I find nested options hard to reason about."* / *"Make this a struct so the
  boolean has a name."* — an unnamed boolean or a doubly-wrapped option is a
  missing type.
- *"I'd expect these variants to stay thin."* — do not widen a shared type for
  one case; find the path where the extra data can live only where it applies.
- *"Could we do this without an allocation?"* — asked specifically about hot
  paths, and not asked elsewhere. Precision about *where* performance matters is
  what stops it from becoming a tax everywhere.
- *"The safety comment should explain the justification, not restate which
  operations are called."* — a comment that paraphrases the code is worthless;
  the comment must contain what the code cannot.
- *"Should we document internals here? They could change."* — resisting
  documentation that accidentally becomes a promise.
- *"Leave a note to switch to the better construct once the minimum version
  allows it."* — deferred improvements get an explicit, findable marker rather
  than being silently forgotten.
- *"Only incremental improvement is needed to land. It does not need to be
  perfect, only better than the status quo."* — stated policy, and visible in
  practice: maintainers land imperfect work and open follow-ups rather than
  holding contributions hostage.
- *"Request changes, do not demand them, and do not assume the contributor knows
  how to add a test or run a benchmark."* — and: mark nits explicitly as
  non-blocking, or fix them yourself while landing.

The through-line: **reviewers spend their attention on names, on unnecessary
state, on things that can drift out of sync, and on whether a claim is true.**
They spend almost none on formatting, because a machine does that.
