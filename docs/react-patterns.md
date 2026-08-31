# React and TypeScript patterns

Language- and framework-specific practice, distilled from performance and
correctness work in large React applications, component libraries, and the
rendering libraries underneath them. Every entry is the *general* idea, not the
case it came from. See [system-architect.md](system-architect.md) for the design
layer and [coding.md](coding.md) for how to work.

The recurring shape: **React makes work invisible, so the job is to know what
causes work and to stop paying for what nobody asked for.**

---

## Know what causes work

Before optimising anything, know which of these you are looking at. They have
different fixes and confusing them wastes days.

1. **Too many renders** — a component re-runs when its inputs did not really
   change.
2. **Too much work per render** — the render itself allocates, computes, or
   walks something large.
3. **Too much committed** — the diff is fine but the DOM mutation or layout it
   triggers is expensive.
4. **Work outside render** — an effect, a subscription, a timer, or
   instrumentation doing more than the frame can afford.

Measure which one it is. The first suspicion is usually partially right and
insufficient: a real investigation reads *"memoising the components helped
somewhat but did not resolve it"*, and then keeps going.

## Do not pay for what was not requested

**Gate optional work on whether anything consumes it.** The single largest win
observed in this space — a thirty-fold speedup — was skipping the construction of
a derived, tracked copy of every result when the optional transform that
consumes it was not supplied. The work had always been done unconditionally
because the feature existed, not because it was in use.

Look for: derived data built for an optional callback, instrumentation collected
when nothing is profiling, formatted strings built for a log level that is off,
tracked or proxied wrappers created for consumers that read the raw value.

**Instrumentation must be off, or bounded, when nobody is observing.** Buffered
performance entries retain everything they reference. Development-only tracing
that copies props keeps those props alive. Emit only while profiling, and clear
what you accumulate.

## Per-item work and accidental quadratics

**If a function called once per item computes something over all items, that is
quadratic.** The fix is almost always to hoist the whole-set computation out and
pass the result in. This is the most common serious performance bug in list-heavy
React code and it does not show up until the list is big — which is exactly when
a user notices.

**Deduplicate idempotent fan-out.** When reading one property has to mark every
item as tracking that property, doing it per read is quadratic; remember that
the property was already broadcast and do it once. Any "notify everyone about
X" that is idempotent should run once per distinct X, not once per caller.

**Special-case the common shape.** A generic recursive comparison that handles
every possible value pays for that generality on every call. Branching to a
direct loop for the shape that actually dominates — usually arrays — is a large
win for a small amount of code.

**Do not allocate per iteration.** A closure created inside a recursive
comparison, a new array from `map` used only to be iterated, an object literal
rebuilt per row. `for...of` over the thing you have beats `forEach` with a fresh
arrow function in genuinely hot code.

**Do not read past the end of an array.** In JavaScript engines this changes the
array's internal representation and deoptimises the surrounding function.
Bounds-check inside the loop rather than relying on `undefined` coming back.

## Memoisation

**Purity is the precondition, not an afterthought.** The reason a per-render,
per-instance function can be cached is that it is a pure function of stable
inputs. Establish that first — if it reads props, or the clock, or module state,
memoising it is a bug rather than an optimisation.

**The highest-value target is a pure function of a stable input called once per
instance per render.** Styling functions, formatter construction, derived
configuration. These produce an identical result for the lifetime of the input
and are rebuilt thousands of times.

**Memoise the expensive thing, not the cheap thing.** `useMemo` around a
primitive comparison costs more than it saves. Reserve it for allocation-heavy
derivations and for values whose identity feeds a dependency array downstream.

**Stable identity is a contract.** When a value is passed to a memoised child,
into a dependency array, or into a context, its identity is part of the
interface. Say so where it is created, because a later "harmless" change from a
hoisted constant to an inline literal silently un-memoises the subtree.

**Memoising components is the blunt instrument, not the first one.** It helps,
it rarely resolves, and it adds a comparison at every render. Prefer fixing the
input that keeps changing.

## Retention and cleanup

**Clear a rollback or snapshot field the moment its purpose expires.** A field
kept "in case we need to revert" retains everything it points at for the lifetime
of its owner. When the operation succeeds, null it.

**Every subscription needs a removal path that runs on the success case.** The
expensive bug in this family: a listener attached to a caller-supplied signal or
event target, removed only from inside the listener. On success it never fires,
so the closure keeps the entire finished result reachable for as long as the
caller holds the source. Prefer a mechanism where removal is *structural* — an
`AbortSignal` passed to `addEventListener`, a scope, a disposer bag — over a
teardown function someone must remember to call.

**Encapsulate a repeated cleanup obligation into a hook or class.** If every call
site has to remember to clear a timer, cancel a request, or release a handle, one
of them will not. Wrap it once so the cleanup is automatic and the obligation
cannot be forgotten.

**Effects clean up in reverse.** Anything an effect starts, its cleanup stops —
including in the case where the effect re-runs because a dependency changed, not
only on unmount.

## Animation and interaction

**Anything per-frame is scaled by elapsed time.** A flat per-tick multiplier —
`speed *= 0.91` — decays twice as fast on a 120 Hz display as on a 60 Hz one,
because it is applied twice as often. Write decay as a power of elapsed time and
velocity as units per second. Every per-frame decay, increment, or ease is a
latent device-dependent bug, and it arrives as "feels wrong on my machine", which
is nearly impossible to act on.

**Fake the clock in tests.** Stub the high-resolution timer so animation and
timing fixtures are deterministic. Every animation and every self-measurement
depends on it.

**Keep pointer and scroll handlers off the render path.** Work in a
high-frequency handler blocks the frame it is in. Coalesce to the next frame,
and read layout once per frame rather than per event.

**Cache derived geometry, and invalidate it explicitly.** Hit testing, bounds,
and spatial lookups recomputed per pointer move dominate canvas-style
interactions. Cache them keyed by the thing they derive from, and be deliberate
about what invalidates the cache.

## State and data flow

**One state, one representation.** A sentinel doing double duty — `null` meaning
both "not loaded yet" and "loaded, nothing found" — will eventually be read as
the wrong one, and the case where it matters is a retry or a remount. Model the
states explicitly; a discriminated union is cheaper than the bug.

**Do not share a counter or a version between two independent concerns.**
Activity in one lane then makes the other look changed, and every consumer of
"has this changed" gets a false positive.

**Derive rather than duplicate.** State that can be computed from other state is
a synchronisation problem you have chosen to have. Store the minimum and compute
the rest during render.

**Lift state to the lowest common owner, not to the top.** State parked above
where it is used re-renders everything in between for no reason.

**A retried operation must be idempotent or keyed.** A retry that re-runs a whole
handler where one write overwrites and another appends will silently accumulate
duplicates. Overwriting writes tolerate retries; appending writes need an
identity that makes the second attempt recognisable as the same attempt.

## Boundaries

**One definition of every shape that crosses a process or bundle boundary.**
Redeclaring a payload type on the receiving side lets the two sides drift with no
type error anywhere — both compile happily until runtime. Generate one side from
the other, or hand-write one declaration that both import.

**Inline the first payload; lazy-load the heavy parts.** Give an embedded view
the state it needs to paint its first frame along with the view itself instead of
making it request it after mount, and defer anything large that is not needed for
that frame. Two independent wins, neither of which changes the architecture.

**Validate at the boundary, then trust inwards.** Parse untrusted input into a
domain type once, at the edge. Downstream code should receive the type, not the
raw payload plus a promise that someone checked it.

**Review the build's view of a change too.** A new file outside a watched
directory silently stops triggering rebuilds; a new import can break tree
shaking; a moved module can leave a bundle entry stale. These do not show up in
the diff.

## TypeScript specifically

**`readonly` is a compile-time claim, not a runtime guarantee.** A readonly-typed
collection can still be mutated by anything holding an untyped reference, and
freezing does not cover every container. If immutability matters at runtime,
enforce it at runtime or keep the value private.

**Prefer discriminated unions over optional-field soup.** Four optional fields
where only certain combinations are legal is sixteen states of which twelve are
bugs. A union of the legal shapes makes the illegal ones unrepresentable and
makes exhaustiveness checkable.

**Avoid `Option<Option<T>>` in any form** — a nullable field of a nullable type,
or a boolean pair standing for three states. Give the states names. When a
function takes a bare boolean, consider a small object or a union of literals so
the call site says what it means.

**Type the boundary, infer the interior.** Annotate exported signatures and
anything crossing a seam; let inference handle locals. Explicit annotations
inside a function are noise that goes stale.

**Do not widen a shared type for one caller.** An extra optional field that only
one consumer sets makes every consumer handle a case that cannot happen for them.
Split the type.

## Doing performance work

**Profile first, and name which of the four causes you found.** The fix for "too
many renders" makes "too much work per render" worse if you guessed wrong.

**Quantify with a real scenario.** "10,000 items, each updated once inside one
batch, median of five runs: 1,444 ms → 46 ms." A number with its scenario is
evaluable; "faster" is not.

**State the expected magnitude honestly.** "This shows up at 1–2% of runtime in
benchmarks, so expect 0.5–1% overall" is a good reason to make a small clean
change and a good reason not to make a large ugly one.

**For a pure performance change, prove the output is unchanged** — identical
rendered result on the full fixture set, not merely a passing test suite.

**Record the numbers where a regression would show up in review.** The cheapest
durable defence is to write the relevant counts into the recorded output of the
test suite, rounded coarsely enough that noise produces no diff. Then a
regression arrives as a text diff beside the behavioural changes, seen by the
same person in the same pass, rather than in a dashboard nobody opens.
