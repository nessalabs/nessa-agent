# 0013. Fetch every agent runtime instead of shipping it

## Purpose

Stop carrying three agents' runtimes inside the application. Fetch what a person
actually uses, reuse what is already on their machine, and say which versions
Nessa supports rather than shipping exactly one.

- **Date:** 2026-09-21
- **Status:** proposed. Reviewed independently and returned with changes; the mechanism below is the one that review chose, not the one first written.
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
   its version is one this build supports. Supported means **between** the floor
   and the pinned version: an older one we still handle, or the one we tested.
   Not "anything newer", for the reason the next section gives — the danger is a
   newer version, and admitting one here would contradict the paragraph that
   refuses it three lines down.
3. **Not present** — offered, and fetched on demand into `~/.nessa/`.

The application drops by 539 MB — the two dependency trees — and a person who
only uses Claude never downloads Codex.

It does not drop to nothing, and an earlier draft of this document said it did.
The runtime directory is 709 MB, of which `node` is 138 MB and the gateway and
its MCP server are another 33 MB. Node runs the adapters, so it stays. What
leaves is the 539 MB that is the agents themselves.

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

- **Floor:** the oldest version whose protocol behaviour we still handle.
- **Pinned:** the exact version this build fetches when nothing usable is
  present, with its digest, as today. It is also the **ceiling**: a runtime
  already on the machine is used when it sits between the two, and a newer one
  is not, because nothing recorded its behaviour.

A person who already runs a newer Opencode keeps using it, and we say plainly
that it is newer than we tested. A person with nothing gets the version we
tested, verified byte for byte. Nessa is not tied to one version forever — each
release moves the pin and may move the floor — and no release ships bytes nobody
checked.

## How it is fetched, which is not what this first proposed

The first draft said: run the same `npm ci`, later, on the machine. That does
not work. The application bundles `node` as a single binary and no npm, npx or
corepack — an independent review confirmed there is nothing in the built app
that can install a package tree, and a desktop application cannot require one
from the person using it.

It also never asked what the 539 MB *is*, and the answer changes the design.
It is two platform-keyed native packages —
`@anthropic-ai/claude-agent-sdk-darwin-arm64` at 190 MB, which contains one
executable and three documents, and `@openai/codex-darwin-arm64` at 277 MB.
That is 467 MB, 87% of it, in exactly the shape this context already fetches:
one platform's binary, named by a pin, checked against a digest. The remaining
72 MB is JavaScript.

So:

- **The native packages are fetched** as pinned, digest-verified archives, by
  the same `PinnedRelease` / `HttpsArchives` / `ManagedRuntimes` path Opencode
  already uses. One mechanism for every agent's binary.
- **The JavaScript stays in the bundle**, installed by `npm ci` at build time
  exactly as today, with those two native packages excluded. At first use it is
  copied out of the bundle into `~/.nessa` — a directory copy, no installer.

No npm on anybody's machine. No install scripts running on it either. The
compiled-in digest is kept rather than traded for a lockfile, so the supply
chain story the review of #44 established is unchanged rather than argued
about. Nothing new is hosted.

The alternative of publishing one archive per agent at release time remains
open and would take the JavaScript out too. It is a bigger change and it adds
artefacts to host; this can be done first and that later, without undoing it.

## What we are not deciding here

Whether to fetch the newest version within a range, verified against the
registry's own integrity metadata rather than a compiled-in digest. That is a
real option, it is what most tools do, and it would let a runtime improve
without a Nessa release. It also gives up the two guarantees above. If we want
it, it should be its own decision with its own review, not a side effect of
unbundling.

## Consequences

- The installed application is 467 MB smaller on disk. The *download* shrinks by
  less, because it is compressed — the disk image is 410 MB today — and an
  earlier draft of this document wrongly claimed the download itself fell by
  539 MB.
- First use of an agent costs a fetch. Today that cost is paid by everyone at
  install time whether they use the agent or not.
- The pin file grows from one agent to three, and so does the work of moving a
  pin: new digests, and contract fixtures re-recorded against the new version.
- A runtime already on the machine is used rather than duplicated, which is what
  someone who already runs these tools would expect.
- `agent_install`'s rationale paragraph is rewritten. It currently explains why
  one agent is special, and none is.
- **Three install paths now exist where there was one, and none of them records
  what it did.** #102 asks for an audit port covering started, verified,
  rejected, replaced and rolled back for a single agent's install. This decision
  triples the surface that lacks one, on a path that writes an executable the
  application later launches. The port should land with this, not after it.
- **Running an installer is an effect at the process boundary**, and
  `AGENTS.md` asks for those behind a caller-owned port with a substitute in
  tests. Whatever mechanism is chosen above, the call out to it is injected
  rather than reached for, so a test can exercise a failing install, a partial
  one, and one that reports success and produces nothing.
- `docs/codebase-structure.md` gains the new module and loses the bundled
  harness paths.
