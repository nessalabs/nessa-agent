# Rust patterns

Language-specific practice, distilled from performance and correctness work in
long-lived Rust systems. Every entry is the *general* idea, not the case it came
from. Read [system-architect.md](system-architect.md) for the design layer and
[coding.md](coding.md) for how to work; this is what to do once you are writing
Rust.

The recurring shape across all of it: **Rust makes costs visible, so the job is
to put each cost where the caller can see it and decide.**

---

## API shape

**Take the most general thing, return the most specific.** Accept `&str` over
`&String`, `&[T]` over `&Vec<T>`, `impl IntoIterator` over a concrete
collection. Return concrete types so callers keep their options. Generality in
the parameter position costs nothing and buys every caller; generality in the
return position leaks your implementation.

**Let the caller allocate.** A function that takes `&mut String` or `&mut Vec<T>`
to write into lets callers reuse a buffer across a loop; one that returns a fresh
`String` forces an allocation per call and hides it. Where both are useful,
provide the writing form and build the allocating form on top of it — never the
reverse.

**Pass context first, not last.** Anything threaded through a call chain — a
handle, a config, a clock — goes in the first parameter position. It reads as
"in this context, do this", and it means adding or removing context does not
churn the interesting arguments.

**Push conditionals up, push loops down.** A function that sometimes does nothing
depending on state it reads itself cannot be reasoned about locally; hoist the
condition to the caller. Conversely, a function called in a loop should usually
take the whole batch, so the per-item cost is paid once.

**Minimise what is `pub`.** Everything public is a promise. Prefer
`pub(crate)`, and make a thing public only when something outside actually needs
it — not because it seems generally useful.

## Types and invariants

**A field with an invariant is private and enforced in the constructor.** If a
field can hold any value without breaking anything, make it public. If it cannot,
document the rule, make the field private, and enforce it in the one function
that can construct the type. The invariant is then verified by reading a single
file.

**Newtype anything with a unit or a meaning.** `SessionId(String)`,
`Millis(u64)`, `Points(f64)`. Primitive obsession is the cheapest possible bug:
the compiler cannot tell you that you passed a width where a height belonged.

**Encode roles in distinct types.** When one underlying structure has two access
disciplines — a producer half usable from one thread and a consumer half usable
from many — give them two names. The rule then appears at every use site and
cannot be violated by accident.

**Exploit forbidden values.** If zero, or empty, or a particular sentinel is not
a legal value in your domain, encode that: offset the representation, or use
`NonZero*`. It shrinks the type, makes `Option<T>` free via niche packing, and
turns an illegal state into a compile-time impossibility.

**Do not let a rare case fatten a shared type.** When one variant of an enum or
one field of a struct is large but rarely used, everything pays for it in every
instance. Box it, or split it into a separate structure that only the rare path
carries. Reviewers of good codebases say this out loud: *I would expect these
variants to stay thin.*

**Size matters when the count is large.** For a type that exists in millions of
instances — cache keys, node ids, per-item metadata — its byte size *is* the
memory profile. Count the bytes, hand-pack bit fields into a single integer if
the win is real, and document the layout where the type is declared.

## Allocation

**Reserve exactly what you write.** A capacity hint that does not match the
actual write is worse than no hint: it either wastes memory or triggers the
reallocation it was meant to prevent. If the size depends on a computation, do
the computation.

**Reuse the buffer rather than freeing it.** Clearing and refilling a collection
keeps its allocation; dropping and recreating it does not. In any loop or
repeated operation, hold the buffer at the outer scope.

**Watch for the defensive copy inside a loop.** The most common accidental
quadratic in Rust is a `clone()` made for a good local reason — ownership,
avoiding a borrow conflict, immutability — sitting in a loop over something that
grows. The fix is usually a small index-or-handle type rather than the value.

**Construct errors lazily.** `ok_or_else`, not `ok_or`; build the error message
inside the closure. An error path that allocates on every success is a tax on the
common case.

**Lazy and memoised beats eager global beats recomputed.** Initialise expensive
state on first use, not at construction — especially when a strategy selector
means most of the alternatives are never used at all. Paying at construction for
something usually unused is the same waste as recomputing, just moved.

## Generics, monomorphisation, code size

**Outline the non-generic body of a generic function.** A generic function is
copied per instantiation. If most of its body does not depend on the type
parameter, move that body into a private non-generic function and leave a thin
generic shim. Compile time, binary size, and instruction cache all improve, and
the change is mechanical.

**Make helper types generic over the minimum.** An intermediate combinator
generic over the *input* type, rather than over the *item* type it actually
handles, multiplies out across every combination. In deeply composed pipelines
this is the difference between a normal build and gigabytes of intermediate
representation.

**Type erasure has a cost — pay it as late as possible.** Putting a check behind
a function pointer or a `dyn` boundary means the optimiser can no longer see
through it, and the trivial check becomes a real call. Do the cheap concrete work
*before* erasing, at the site where the type is still known.

**Mark cold paths cold.** `#[cold]` and `#[inline(never)]` on rare branches —
error construction, slow-path refills, panics — keep them from being duplicated
into every call site. This matters most inside functions the compiler inlines
aggressively, where the rare path otherwise multiplies across the whole binary.

**`#[inline]` on small leaf implementations that cross a crate boundary.** Trait
implementations for simple in-memory types are the clearest case: without the
attribute a downstream crate cannot inline them, and it loses bounds-check
elision and dead-branch removal at every call site. Do not scatter `#[inline]`
generally — put it on small functions where cross-crate visibility is the point.

## Concurrency

**Move the shared counter from the frequent event to the rare transition.** If
every unit of work touches a shared atomic, that atomic is your bottleneck. Ask
what you are really trying to know; often a count of *active workers*, updated
only when a worker sleeps or wakes, answers the same question with a fraction of
the contention.

**An immutability proof lets you skip synchronisation entirely.** If a target
provably cannot change or be invalidated, the lock protecting the reader is
unnecessary. Take the fast path when the proof holds, and fall back to acquiring
in the canonical order when it does not — never to a different order.

**State the access rules per field.** For any structure touched by more than one
thread, write a comment naming, for each field, who may read it, who may write
it, and under what condition. This is the only place that information can exist,
because the compiler cannot express it. Number the rules so review comments can
cite them.

**Every queue, channel, and buffer has a bound.** Unbounded means a memory leak
that has not happened yet. Decide the behaviour at the bound — block, drop
oldest, drop newest, error — and document it.

**Check whether the expensive signal is needed before sending it.** Waking a
parked worker, scheduling a re-render, invalidating a cache — these often happen
unconditionally when the state already guarantees they are redundant.

**Route synchronisation primitives through one internal module.** Import your
atomics, locks, and cells from an internal shim that re-exports either the real
ones or instrumented ones under a build flag. Then exhaustive concurrency testing
becomes a build configuration rather than a refactor. Shrink capacity constants
under that flag so the checker terminates.

## Errors, panics, and failure

**Panics are API.** If a function can panic, document the exact condition,
annotate it so the panic is attributed to the caller, and add a test asserting it
panics where it should. Collect those tests in one place so the panic surface is
reviewable as a set.

**Convert internal arithmetic failures into documented ones.** An overflow deep
in a constructor produces an incomprehensible panic. Check the boundary condition
explicitly, panic with a message that names the parameter, and document that the
function can panic on absurd input.

**Guard the algorithm's forbidden inputs at construction.** Many algorithms have
inputs that are legal to the type system and fatal in practice — a zero seed for
a generator that then produces only zeros, an empty set where the loop assumes
one element. Reject them where the value is built.

**Consider unwinding in unsafe and resource code.** Any call that can panic
between "took ownership" and "stored it safely" is a leak or a double-free
waiting to happen. Order operations so the fallible step happens before you are
holding something that needs cleaning up.

**Prefer early returns.** `let Some(x) = .. else { return }` and `?` keep the
happy path at one indentation level and let the compiler find dead branches.

## Async

**A `Future` does nothing until polled.** Constructing one is free; not awaiting
one is usually a bug. Anything that must happen regardless of whether the caller
waits belongs in a spawned task, not in a future you hand back.

**Say what happens on cancellation.** Any `async fn` that can be dropped mid-way
must document whether partial work is visible, and every `select!` arm is a
cancellation point. Cancellation safety is documentation, not an implementation
detail.

**Never block in async context.** Filesystem calls, heavy computation, and
synchronous locks held across an `await` stall the whole executor. Move them to
a blocking pool, and treat "does this ever block?" as a review question for every
new dependency.

**Do not hold a lock across an `await`.** It is the async equivalent of holding
a lock across an I/O call, and it converts a fast mutex into a source of
deadlock.

## Testing and tooling

**Test through the public surface only.** No test-only visibility, no test-only
constructors. If a state is unreachable through the real API, it should not
exist. When a private helper genuinely cannot be reached, say so in the change
rather than inventing a door.

**Compile-fail tests protect type-level invariants.** When you encode a rule in
the type system, add a test asserting that the illegal usage *fails to compile*,
with a snapshot of the error. Without it, a later refactor can quietly make the
illegal state legal and nothing goes red.

**Run the checkers, not just the tests.** Undefined-behaviour checking,
sanitisers, the exhaustive concurrency checker, a fuzzer for anything parsing
untrusted input, and a compatibility check on the public interface. Each is a
separate narrow job; a single "test" job that does everything tells you only that
*something* broke.

**Name the change that introduced a regression** in the fix. It costs one blame
and it makes the history queryable.

## Doing performance work

**Profile first, and state the expected magnitude.** "This function shows up at
1–2% of runtime, so this is worth about 0.5–1% overall" is an honest, evaluable
claim. It also tells the reviewer how much complexity the change may justify.

**Quantify the result in the change description**, with the benchmark and the
machine. A performance claim without a number is a rumour.

**For a pure performance change, prove the output is unchanged bit for bit** —
on the full fixture set and on pathological inputs, not just "tests pass".

**Benchmark the contention, not only the throughput.** A change that improves
single-threaded speed can lose badly under concurrency. If the code is shared,
the benchmark must exercise it from several threads.

**Revert an optimisation you cannot maintain.** The clearest example of good
judgement seen in this space: a subtle optimisation kept producing fuzzer
findings, and the maintainer removed it entirely with the reason "this is too
subtle and I do not have time to fix it properly". An optimisation nobody can
safely modify is a liability whatever its benchmark says.
