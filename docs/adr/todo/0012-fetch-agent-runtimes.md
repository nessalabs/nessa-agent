# 0012. Fetch every agent runtime instead of shipping it

## Purpose

Stop carrying three agents' runtimes inside the application. Fetch what a person
actually uses, reuse what is already on their machine, and say which versions
Nessa supports rather than shipping exactly one.

- **Date:** 2026-09-21
- **Status:** proposed.
- **Follows:** [#104](https://github.com/nessalabs/nessa-agent/issues/104), which joined an installed runtime to a launch, and [#129](https://github.com/nessalabs/nessa-agent/issues/129), which showed the reason Opencode was treated differently was not true.

## What this is about

Today the application ships two of the three agents and fetches the third:

| Agent | How it arrives | Size |
| --- | --- | --- |
| Claude | bundled | 244 MB |
| Codex | bundled, including the Codex CLI | 295 MB |
| Opencode | fetched on demand, digest-verified | 144 MB |
| | **runtime total in the app** | **709 MB** |

Every one of them comes from the same npm registry. Two are vendored at build
time by `npm ci`; one is fetched by `nessa install-agent`. So the difference is
not what they are, only when we get them.

That asymmetry was justified by a claim that has since been disproved: Opencode
was "the agent a first-time user can reach with nothing signed in". It is not —
its free models are refused outside OpenCode's own app. All three agents need
the person's own account.

## What we decided

Fetch all three. Nothing agent-shaped ships inside the application.

A runtime is resolved in this order, at every start:

1. **Already installed by Nessa** — the store answers, and that is the launch.
2. **Already on the machine** — a runtime the person installed themselves, if
   its version is one this build supports.
3. **Not present** — offered, and fetched on demand into `~/.nessa/`.

The application drops from 709 MB of runtime to none, and a person who only
uses Claude never downloads Codex.

## The part that needs care

"Tie it to a range rather than a version" reads as one decision and is two, with
different costs.

### A floor is cheap and correct

*Refuse anything below version X.* This costs nothing and prevents a real
failure: a runtime already on the machine that is too old to speak the protocol
we expect. This is what makes step 2 above safe, and it is what a range should
mean here.

### A ceiling is not optional, and this is the surprising part

*Accept anything at or above version X* would break two guarantees this
repository spent real effort establishing.

**The recorded contract.** Every adapter is verified against fixtures recorded
from one exact version — `codex-acp` 1.12.0, Opencode 1.18.31. Review of those
adapters found behaviour that differs between versions and is invisible until it
runs: Codex answering `startedNewTurn` where the worker accepts only `injected`
or `promptRequired`, Opencode opening every session in a mode the profile did
not expect. A floor does not protect against any of that, because the danger is
a **newer** version, not an older one. Accepting "anything recent" means running
against wire behaviour nobody recorded.

**The verified bytes.** `agent_install` compares a download against a SHA-256
compiled into the build. A version that did not exist when this build was made
cannot have a digest in it. Fetching "the newest in the range" means trusting
the registry at fetch time instead of the build at review time — which is what
`npm ci` does, and weaker than what we do now.

### So the range has two ends

- **Floor:** the oldest version whose protocol behaviour we still handle. Used
  to decide whether a runtime already on the machine is usable.
- **Pinned:** the exact version this build fetches when nothing usable is
  present, with its digest, as today.

A person who already runs a newer Opencode keeps using it, and we say plainly
that it is newer than we tested. A person with nothing gets the version we
tested, verified byte for byte. Nessa is not tied to one version forever — each
release moves the pin and may move the floor — and no release ships bytes nobody
checked.

## What we are not deciding here

Whether to fetch the newest version within a range, verified against the
registry's own integrity metadata rather than a compiled-in digest. That is a
real option, it is what most tools do, and it would let a runtime improve
without a Nessa release. It also gives up the two guarantees above. If we want
it, it should be its own decision with its own review, not a side effect of
unbundling.

## Consequences

- The download is 709 MB smaller, and a person downloads only the agent they
  pick.
- First use of an agent costs a fetch. Today that cost is paid by everyone at
  install time whether they use the agent or not.
- The pin file grows from one agent to three, and so does the work of moving a
  pin: new digests, and contract fixtures re-recorded against the new version.
- A runtime already on the machine is used rather than duplicated, which is what
  someone who already runs these tools would expect.
- `agent_install`'s rationale paragraph is rewritten. It currently explains why
  one agent is special, and none is.
