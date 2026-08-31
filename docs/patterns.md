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

**Refactor, then test, then change — as three commits.** A recurring shape in
codebases with a long memory, visible directly in the commit sequence: first
*split the logic out so it is reachable from a test*, with the message saying
"no functional change"; then *add the tests, whose reference output records the
current behaviour including the wrong parts*; then *change the behaviour*. Each
commit is separately reviewable and separately revertible, and the middle one
makes the third one's diff a list of exactly which behaviours changed.

**Name the commit that introduced the defect.** The strongest convention seen:
every regression fix carries a `Fixes: <hash> ("<original subject>")` line
pointing at the change that caused it. It costs one `git blame` and it turns the
history into something you can actually query — what did this change break, how
long did it take to notice, which areas keep regressing.

**Big structural changes are made *ahead of* the feature that needs them.** One
of the clearest examples: a ~2900-line refactor whose entire stated motivation
was that a *future, not-yet-written* feature would swap out an object at
checkpoint time, invalidating every outstanding reference to it. The refactor
landed alone, with no feature attached. The description named two strategies and
the rule for choosing between them — encapsulate the logic into the owner where
possible, and where it is not possible *because external extensions need it*,
introduce an explicit handle. Notice that the reason encapsulation fails is
precisely vertical isolation: the core cannot write the functions in advance
because it does not know what the product-specific code wants to do.

Reviewers on that change did something worth copying: they reasoned forward to
the unbuilt feature. *"Thinking ahead to the swapped index — will this still be
correct if we revert an append after the object was replaced?"* And they caught
a silently narrowed lock scope: *"this returns the pinned items but doesn't hold
the list lock, whereas before we held it for the whole loop."* Lock scope is
exactly the kind of thing a refactor changes by accident and a diff does not
make obvious.

**A mechanical mega-change is justified by the test suite, not by review.** A
change replacing a hand-written interface layer with a specification plus code
generation — sixteen thousand lines added, nineteen thousand deleted, 112 files —
had a three-sentence description whose substance was: *if there are any
unexpected divergences, the existing tests will catch them.* That is the only
honest way to review a change of that size, and it is only available to a
codebase that earned it first. It is also the strongest possible argument for
generating repetitive code from a spec rather than maintaining it by hand.

**Cleanup has an owner on each side of the boundary.** From a change making
aborted bulk writes clean up after themselves: the writers remove their own
un-finalised output, and the transaction removes the finalised output. Stated
explicitly, because otherwise both sides assume the other did it. Reviewers on
that change pushed twice on API shape — *"make this a flag, or we will have
`TryRemoveNonEmptyDirectory` next"* and *"this is added in many different places,
there is probably a less error-prone way"* — which is the right instinct: a
cleanup obligation repeated at every call site will be forgotten at one of them.

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

## Performance work, specifically

**Quantify the claim in the description.** The good performance changes lead
with numbers: *peak memory allocation down 5–15%, code generation time down
30–70% depending on payload*. Not "this is faster". A number is falsifiable and
a reviewer can decide whether it is worth the complexity; "faster" cannot be
argued with and therefore cannot be evaluated.

**Prove the output is unchanged, bit for bit.** The safety net for a pure
performance change is not "the tests pass" — it is *all fixtures produce
byte-identical output, and I ran it against real codebases and pathological
inputs and confirmed byte-identical output there too*. When a change is supposed
to alter only how long something takes, equality of output is the whole
correctness argument, and it is cheap to check exhaustively.

**Record performance statistics in test baselines.** The most transferable idea
found: a test harness that automatically writes counters — how many symbols,
how many nodes, how much work — into the recorded baseline output whenever the
number is interesting, **rounded to coarse intervals so that ordinary noise does
not produce a diff**. A performance regression then shows up in review as a text
diff in a file, next to the behavioural changes, reviewed by the same person in
the same pass. No dashboard, no separate job, no one remembering to look.

**"Accidentally quadratic" is the failure mode to hunt.** One change found a
code generator that deep-cloned a syntax tree to record what it had replaced —
correct, local, and invisible in review, and quadratic in the size of the input.
The pattern to watch for: a defensive copy inside a loop, made for a *local*
reason (ownership, immutability, avoiding aliasing), where the loop is over
something that grows.

**Laziness beats eagerness beats recomputation.** A change hoisted a repeatedly
constructed lookup table into a constant. The reviewer's correction is the
lesson: don't make it an eager global — make it *memoised and lazy*, and mark
the initialiser so a bundler can drop it entirely when unused. Eager work at
startup is still work; the goal is *once, and only if needed*.

**A readonly type is not a runtime guarantee.** From the same review: the hoisted
table was typed as immutable, and a reviewer noted that the type does not prevent
mutation and the obvious runtime freeze does not work on that container. Shared
mutable state that is only *typed* as immutable is shared mutable state.

**Hunt retention, not just allocation.** A subtle and expensive class of bug: an
event listener registered on a caller-supplied cancellation signal, removed only
from inside the listener itself. On the success path the signal never fires, so
the listener stays attached and its closure keeps the *entire finished result*
reachable for as long as the caller holds the signal. The fix was structural
rather than bookkeeping — bind the listener to a lifetime that ends when the work
does, so the runtime removes it, and nothing has to remember to. The reviewer
proposed exactly that, and the author's reply is the tell: *"that made the
cleanup much simpler, and it automatically handles the cancelled case too."*

**Any subscription without a removal path is a leak.** Generalise the above:
whenever you attach a callback to something whose lifetime you do not control,
the removal must be structural — tied to a lifetime, a scope, a guard — not a
teardown function someone must remember to call on every exit path, including the
successful one.

**Measure to schedule, and watch the measurement.** A build pipeline that
assigned work to workers round-robin was changed to shard by *measured* duration,
with the measurements written out by the build itself and fed back on the next
run. Two details make it good rather than clever: the measurement is written
fresh each time rather than merged, so retired work drops out instead of
accumulating; and a diff against the previous measurement is logged **so that
the team can notice if run-to-run variance grows large enough that the heuristic
has stopped working**. Build the signal that tells you your optimisation has gone
stale.

**Untrusted inputs must not be able to poison a cache.** In that same change:
only trusted runs may write the shared measurement cache, while untrusted runs
may read it. A shared cache that anything can write is an injection point.

---

## Interface and UI systems

**Anything per-tick must be scaled by elapsed time.** A camera-throw that decayed
with a flat multiplier once per frame lost speed twice as fast on a 120 Hz
display as on a 60 Hz one — the decay was `f^(2n)` where it should have been
`f^n` over the same wall-clock time. The fix expresses friction as a power of
elapsed time rather than a per-frame constant. **Every per-frame decay,
increment, or velocity is a latent device-dependent bug** until it is written in
terms of elapsed time, and the bug reports arrive as "feels wrong on my machine",
which is nearly impossible to act on.

**A sentinel that means two things will eventually mean the wrong one.** From a
persistence change: a null field was doing double duty as "never looked up" *and*
"looked up, nothing usable found". Under retry those two cases needed opposite
behaviour, and the result was a write that could be silently skipped. Give each
state its own representation. The same change had a second instance of the same
disease: one counter shared between two independent lanes of activity, so
activity on one lane made the other look changed.

**A retried operation must be idempotent, or it must be keyed.** In that case the
retry re-ran the whole body, where one write overwrote and another appended under
a fresh timestamped key — so a retry silently accumulated duplicates. Overwriting
writes tolerate retries; appending writes need an identity that makes the second
attempt recognisable as the same attempt.

**Do not duplicate a shape across a process boundary.** Observed on a change that
passed initial state from a host into an embedded view: the payload type was
redeclared on the receiving side instead of imported from the shared definition,
so the two could drift apart with no type error anywhere. If two sides of a seam
must agree on a shape, exactly one definition exists and both import it.

**Inline the first payload, lazy-load the heavy parts.** The load-performance
pattern for an embedded view: pass the initial content in with the view itself
rather than making it ask for it afterwards, and defer anything large that is not
needed to show the first frame. Two independent wins — one round trip removed,
one parse deferred — and neither changes the architecture.

**Fake time in tests.** A UI test suite that stubs the high-resolution clock so
fixtures are deterministic: the same seam discipline as anywhere else, applied to
the one dependency every animation and every performance measurement has.

**Coverage gaps are found by writing tests, and what they find gets filed.** A
change that took a geometry library's line coverage from 31% to 97% surfaced three
behaviours that looked like bugs — and each was **filed as its own issue rather
than fixed inside the test change**. The test change stayed a test change, and the
bugs got their own discussion, their own fix, and their own regression test.

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
- *"I don't think that is fixed yet."* — the single most valuable review
  behaviour observed: a reviewer re-derived the failure path after the author
  said it was handled, walked through the exact interleaving, and was right. The
  author's reply — *"you're right, it wasn't fixed; thanks for pushing back"* —
  is what a healthy review culture sounds like. **An author's claim that
  something is handled is a hypothesis, not evidence.**
- *"Can you expand on why this only handles the committed case?"* — asking who
  owns the other half, rather than assuming it is covered.
- *"This is added in many different places — there is probably a less
  error-prone way."* — a repeated obligation at every call site is a design
  problem, not a diligence problem.
- *"Make it a flag, or we will have a fourth variant of this function next
  month."* — resisting API surface growth by anticipating the sequence.
- *"Does this handle circular references?"* / *"What happens under load in the
  degenerate case?"* / *"Does this need to save and restore the previous value
  if this is re-entered?"* — the three questions asked of any new cache, buffer,
  or shared slot: cycles, adversarial size, re-entrancy.
- *"Make it lazy and memoised rather than an eager global, and mark it so it can
  be dropped when unused."* — precision about *when* work happens, not just how
  much.
- *"This new file lives outside the watched directory, so editing it will not
  trigger a rebuild."* — reviewing the build's view of the change, not only the
  code.
- *"Only incremental improvement is needed to land It does not need to be
  perfect, only better than the status quo."* — stated policy, and visible in
  practice: maintainers land imperfect work and open follow-ups rather than
  holding contributions hostage.
- *"Request changes, do not demand them, and do not assume the contributor knows
  how to add a test or run a benchmark."* — and: mark nits explicitly as
  non-blocking, or fix them yourself while landing.

The through-line: **reviewers spend their attention on names, on unnecessary
state, on things that can drift out of sync, and on whether a claim is true.**
They spend almost none on formatting, because a machine does that.

Reading a wider sample sharpens the last one. *Whether a claim is true* is not
mostly about documentation — it is about the author's assertion that a case is
handled. The highest-value comments in every codebase examined are the ones where
a reviewer took a stated guarantee, reconstructed the path themselves, and found
it did not hold: a lock quietly narrowed by a refactor, a retry that reads back
what its own failed attempt wrote, a sentinel standing for two different states,
a listener with no removal path on the success case. None of these are visible in
a diff. All of them are found by someone asking *how does this behave when it is
interrupted, repeated, or re-entered* — and then not accepting the answer without
walking it through.
