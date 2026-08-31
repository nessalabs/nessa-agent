---
name: coding
description: >
  How to write code in this repository. Adopt this before making any code
  change — a feature, a fix, a refactor, or a review. It covers the working
  method; the design layer is in system-architect.md and the language-specific
  practice is in rust-patterns.md and react-patterns.md.
---

# Coding

The three layers, and when each applies:

| Layer | Document | When |
| --- | --- | --- |
| **How to think about the system** | [system-architect.md](system-architect.md) | Designing, structuring, or reviewing anything that spans more than one file |
| **How to work** | this document | Every code change |
| **What to do in the language** | [rust-patterns.md](rust-patterns.md), [react-patterns.md](react-patterns.md) | While writing Rust or React/TypeScript |
| **How to find and prove things** | [method.md](method.md) | Investigating a bug, chasing a performance problem, or deciding what to automate |
| **Techniques worth stealing** | [patterns.md](patterns.md) | Reference, when a specific problem matches |

If you are writing in a language with no document here, the method below still
applies in full. Follow the surrounding code's idiom and say plainly that you
had no language-specific guidance to lean on.

---

## 1. Before writing anything

**Read enough to be boring.** Find two or three places that already do something
like what you are about to do, and match them — naming, error handling, file
layout, test style, comment density. Code that reads like the code around it is
cheaper for everyone forever. A change that introduces a second style has to
justify itself.

**Name the thing in the product's words.** If you cannot name the concept without
inventing a word, you have not found it yet. Go find it before you write a type.

**Answer four questions.** They take two minutes and they prevent most rework:

1. *Who has the information needed to make this decision?* Put the responsibility
   there — not where it is convenient to call from.
2. *What must always be true?* Name the invariant and the function that enforces
   it.
3. *What happens if this stops halfway, runs twice, or races?* Design for that
   before the happy path.
4. *How does this get deleted?* If the answer touches more than this module plus
   one adapter, the boundary is wrong.

**Check whether the change belongs at this altitude at all.** A feature that
needs a new parameter on a core type, a new branch in a core function, or a flag
threaded through is usually a sign that the capability should be composed on top
rather than absorbed into the middle.

## 2. While writing

**Write the smallest thing that makes the requirement true.** Not the framework
for a family of things like it. You will know the right abstraction on the third
instance, and you will be wrong about it on the first.

**Do not build what nobody asked for.** No configurability, no extension point,
no generality, no layer that only forwards. A parameter with one caller is a
constant that has not admitted it yet.

**Extract on the second use, not the first** — and only when the two uses are the
same *idea*, not coincidentally the same lines. A block, a local, or a comment is
a cheaper abstraction than a function.

**Make the control flow visible.** Early returns, conditions hoisted to the
caller, no hidden global state deciding whether a function does anything. A
reader should be able to understand one file without holding the rest of the
system in their head — that is the scarce resource you are protecting.

**Put costs where the caller can see them.** Allocation, blocking, I/O, and
retries should be visible at the call site or named in the signature. A function
that silently does an expensive thing is a landmine.

**Comments say why.** The code says what. A comment earns its place by capturing
what the reader will not have: the constraint, the surprise, the thing you tried
that did not work, the issue that caused this shape. If you are about to
paraphrase the line below, delete the comment.

**Correct the name in the same change that invalidates it.** Renaming is cheap.
Living with a name that no longer describes the thing is not.

## 3. Failure first

Design the interrupted case before the happy path — the happy path will be fine.

- **One state, one representation.** A sentinel meaning two things will be read
  as the wrong one, and the case where it matters is always a retry or a restart.
- **Cleanup is structural, never remembered.** Tie removal to a scope, a guard,
  or a lifetime — not to a teardown call on every exit path, because the path
  that gets forgotten is the successful one.
- **Idempotent, or keyed.** Assume anything can run twice. Overwriting
  operations tolerate retries; appending ones need an identity that makes the
  second attempt recognisable.
- **Every queue, buffer, and retry loop has a bound** and a stated behaviour at
  the bound.
- **Write the durable fact before announcing it**, and announce before acting on
  it.
- **Decide fatal versus survivable, and be consistent with the code around you.**

## 4. Tests

**A change either adds behaviour or fixes broken behaviour. Both have a test
that would have failed before.** If you cannot write one, say so in the change
and say why.

- **Through the public surface only.** No test-only visibility, no test-only
  constructor. If a state is unreachable through the real API, it should not
  exist.
- **Real dependencies over mocks.** Mock only what you cannot run. Doubles for
  your own domain test your doubles.
- **Determinism is injected.** Clock, randomness, ordering, I/O. A flaky test is
  worse than no test — it teaches the team that red does not mean broken.
- **Poll with a timeout, never sleep a fixed duration.**
- **Minimal setup.** Strip the fixture to exactly what the assertion needs;
  anything else is misdirection for whoever debugs it later.
- **Name the test after the behaviour**, in domain language.
- **Do not test private methods or internal call order.** Those are the things
  you most want to be free to change.
- **When writing tests surfaces a probable bug, file it separately.** Do not fix
  it inside the test change.

## 5. Performance

Do not sprinkle it. Locate it.

0. **Reproduce it first.** A fix for something you cannot reproduce is a guess
   with a diff attached. See [method.md](method.md) for the full loop.
1. **Profile before changing anything**, and name what you found. In UI work,
   distinguish too many renders, too much work per render, too much committed,
   and work outside render — the fixes are different and guessing wrong makes it
   worse.
2. **State the expected magnitude.** "This is 1–2% of runtime, so expect 0.5–1%
   overall" tells the reviewer how much complexity the change may justify.
3. **Quantify the result with its scenario.** A number with the workload attached
   is evaluable; "faster" is not.
4. **For a pure performance change, prove the output is unchanged** — identical
   results on the full fixture set and on pathological inputs, not merely a
   passing suite.
5. **Hunt the accidental quadratic first.** A per-item function computing
   something over all items; a defensive copy inside a growing loop. These
   dominate micro-optimisation by orders of magnitude.
6. **Prefer once-and-only-if-needed** over eager, and eager over recomputed.
7. **Revert an optimisation you cannot maintain.** Whatever the benchmark says,
   code nobody can safely modify is a liability.

Language specifics: [rust-patterns.md](rust-patterns.md#doing-performance-work),
[react-patterns.md](react-patterns.md#doing-performance-work).

## 6. Shaping the change

**One reason per change.** A refactor and a behaviour change do not travel
together — split them so each can be reviewed, reverted, and bisected alone.

**Refactor, then test, then change — as three commits.** Extract the logic so it
is reachable from a test, saying "no functional change". Add tests whose recorded
output captures current behaviour, *including the parts that are wrong*. Then
change the behaviour, so the third diff is a precise list of what changed.

**Land structural work ahead of the feature that needs it, alone.** A refactor
motivated by a capability that does not exist yet is reviewable on its structure
and revertible for free. Bundled with the feature it is neither.

**Land a large feature as inert infrastructure first** — types, wiring, and the
gate, doing nothing and unable to affect existing behaviour — then one increment
at a time.

**Keep it small.** Under a couple of hundred changed lines where you can. Review
quality collapses past that and the reviewer starts skimming without admitting
it.

**Write the description with two headings**: *why*, linking the report or the
prior attempts, and *what*, as numbered steps mapped to commits. Say what you
deliberately did *not* do and why — it is what stops the next person from
"tidying" the code into the shape you already rejected.

**Name the change that introduced a defect** when fixing a regression.

**Update the prose the change invalidates**, in the same change. Grep for the
term. A stale map is worse than no map because people trust it.

## 7. Reviewing — including your own work

Approve when the change **definitely improves the overall health of the system**,
even if it is not perfect. Perfect is not the bar; withholding approval in
pursuit of it costs more than the imperfection.

Attention in this order: is it the right problem → is the design right → is it
correct → is it tested → is it readable → are the names the product's names → is
it tidy. Never lead with tidy.

Ask these, of your own diff first:

- Does this point a dependency the wrong way, or reach past a boundary?
- Does it put a rule in the layer that does not own it?
- Did a refactor quietly narrow a lock, a scope, or a guard?
- Is there state here that exists only to serve a speculative accessor?
- Is an obligation repeated at every call site instead of encapsulated once?
- Do two things now have to be kept in sync by hand?
- Is a claim in the description actually true — walk the interrupted, repeated,
  and re-entered paths yourself rather than accepting the answer.

**Mark optional feedback `Nit:`.** Everything without it is expected to be
addressed or argued with; the ambiguity is what makes review feel adversarial.

**Facts, then principles, then the author's preference.** "I would have done it
differently" is not a review comment.

**A stated claim is a hypothesis, not evidence.** The highest-value review
comment in every codebase worth imitating is some form of *"I do not think that
is actually fixed"*, followed by the reviewer walking the path themselves.

## 8. Finishing

- The change does one thing, and the message says which.
- A test fails without it.
- It touches the number of files it *should* touch. If a routine change touched
  four modules, that is a boundary problem, not a big feature.
- No new absence-invariant was violated (see
  [codebase-structure.md](codebase-structure.md)).
- Anything expensive to reverse has a written note or an
  [ADR](adr/README.md).
- Names match the domain language, including in tests.
- Report what actually happened. If tests fail, say so with the output. If part
  of the scope was skipped, say which part and why — scaling the work down is
  not your call to make silently.
