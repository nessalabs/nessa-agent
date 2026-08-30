# Testing strategy

Tests exist to make change safe and to give early feedback on coupling. A test
that does neither is overhead. See §11 of [system-architect.md](system-architect.md)
for the reasoning; this is the operational version.

## What gets a test

| Situation | Test |
| --- | --- |
| New behaviour | A test that fails without the change |
| Bug fix | A regression test that fails on the old code |
| Domain rule or invariant | Unit test, real objects, no doubles |
| Use case over real infrastructure | Integration test against the real dependency |
| Contract between modules | Consumer-owned test on the event/DTO shape |
| Architectural absence | Structure test over the module graph |
| Performance claim | Benchmark, run in CI, with a recorded baseline |
| Pure rendering | Nothing, usually. Assert behaviour, not markup |

## Rules

**Public surface only.** No test-only visibility, no test-only constructors. If
a state is unreachable through the real API, it should not exist. Building
fixtures by calling the same methods production calls is not friction — it is
the test proving the API is usable.

**Real dependencies over mocks.** Mock only what you cannot run: a third-party
service, a paid API, hardware you do not have. Mocking your own domain tests
your mocks. For anything with a local equivalent (a temp directory, an in-memory
store honouring the same contract), use the equivalent.

**Determinism is injected, never ambient.** Clock, randomness, scheduling, and
ordering arrive as parameters. A test that passes 99 times out of 100 has
already failed — it has taught the team that red does not mean broken.

**Async assertions poll, never sleep.** Sample the observable state until the
condition holds or a timeout fires. A fixed `sleep` is either slow or flaky, and
usually becomes both.

**One behaviour per test, minimal setup.** Strip the fixture down to exactly
what the assertion needs. Anything left in that does not affect the outcome is
misdirection for whoever debugs it later.

**Name the test after the behaviour**, in domain language: what it does, under
what condition, with what result. `test_handler_2` is a note that says "I did
not know what I was asserting".

## What not to test

- Private methods and internal call sequences. These are the things you most
  want to be free to change; a test on them is a refactoring tax.
- Framework behaviour. The UI library renders; assume it.
- Generated or trivially derived code.
- Exact markup or pixel output, except where a rendering *is* the product
  behaviour and a snapshot is cheaper than a description.

## Structure tests

Once there is more than one context, add a test that reads the module graph and
fails on a violated absence from [codebase-structure.md](codebase-structure.md).
This is the only way an invisible rule stays true. It is cheap to write and it
converts a recurring review comment into a compiler error, which is exactly the
trade you want: reviewers spend attention on design, machines spend it on rules.

## Test placement

- Unit tests live beside the code they test.
- Integration tests live in their own tree and only use public entry points —
  which is what makes them integration tests rather than large unit tests.
- Fixtures and builders are shared *within* a context, never across contexts. A
  shared fixture across boundaries is a shared kernel with worse ergonomics.
