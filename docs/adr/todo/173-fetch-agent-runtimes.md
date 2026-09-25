# 173. Fetch every agent runtime instead of shipping it

## Purpose

Stop carrying three agents' runtimes inside the application. Fetch what a person
actually uses, reuse what is already on their machine, and say which versions
Nessa supports rather than shipping exactly one.

- **Date:** 2026-09-21
- **Status:** proposed — reviewed independently and returned with changes; the
  mechanism below is the one that review chose, not the one first written, and
  nothing here is implemented yet.
- **Follows:** [#104](https://github.com/nessalabs/nessa-agent/issues/104), which joined an installed runtime to a launch, and [#129](https://github.com/nessalabs/nessa-agent/issues/129), which showed the reason Opencode was treated differently was not true.

## What this is about

Today the application ships two of the three agents and fetches the third:

| What | How it arrives | Size |
| --- | --- | --- |
| Claude's runtime | bundled | 244 MB |
| Codex's runtime, including the Codex CLI | bundled | 295 MB |
| `node` | bundled | 138 MB |
| The gateway and its MCP server | bundled | 33 MB |
| | **`Contents/Resources/runtime`** | **709 MB** |
| Opencode's runtime | fetched on demand, digest-verified | 144 MB, not in the app |

The last row sits outside the total on purpose: Opencode is the one thing here
the application does not carry. The rows are rounded, which is why they sum to
a megabyte more than the 709 MB `du` reports for the directory.

All three agent runtimes come from the same npm registry. Two are vendored at
build time by `npm ci`; one is fetched by `nessa install-agent`. So the
difference is not what they are, only when we get them.

That asymmetry was justified by treating Opencode as the agent a first-time user
could reach with nothing signed in. Nessa's supported packaged profile instead
starts on the metered MiniMax M3 Zen model and requires a saved API key. The
runtime-delivery decision does not rely on observed behavior of free catalogue
entries; all three supported profiles require the person's own account.

## What we decided

Fetch all three. No agent's executable ships inside the application.

A runtime is resolved in this order, at every start:

1. **Already installed by Nessa** — the store answers, and that is the launch.
2. **Already on the machine** — a runtime the person installed themselves, if
   its version is one this build supports. Supported means **between** the floor
   and the pinned version: an older one we still handle, or the one we tested.
   Not "anything newer"; the next section is about why a newer version is the
   dangerous one.
3. **Not present** — offered, and fetched on demand into `~/.nessa/`.

The application drops by 467 MB, and a person who only uses Claude never
downloads Codex.

It does not drop to nothing, and an earlier draft of this document said it did.
The runtime directory is 709 MB. `node` is 138 MB of that and it runs the
adapters, so it stays; the gateway and its MCP server are another 33 MB and are
Nessa's own. The two agent trees are the remaining 539 MB, and of that the
467 MB that is native executables is what leaves. The other 72 MB is
JavaScript, and the next-but-one section explains why it is cheaper to keep it
where it is. An earlier draft claimed the whole 539 MB, which the mechanism it
described would have removed and this one does not.

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

A person who already runs an Opencode inside the range keeps using it rather
than downloading a second copy. A person whose own copy is newer than the pin
gets ours alongside theirs, and we say why: theirs is newer than anything we
recorded, and we would be guessing about the wire. A person with nothing gets
the version we tested, verified byte for byte. Nessa is not tied to one version
forever — each release moves the pin and may move the floor — and no release
ships bytes nobody checked.

Both ends are comparisons between two versions, and that is a domain rule with
one trap in it. Versions are strings, and as strings `0.9.0` sorts above
`0.76.0` — a build whose floor was `0.76.0` and compared them as text would
accept a runtime nearly seventy releases too old and find out when the protocol
did not match. So the comparison reads components as numbers. Two spellings are
not orderable that way at all, and the rule must say so rather than guess:
build metadata (`1.2.0+build1`, which every version scheme ignores for
precedence, and which a naive numeric comparison sorts *below* `1.2.0`) and
pre-release tags against each other (`1.2.0-alpha` versus `1.2.0-rc1`, which
reduce to the same numbers). A range can afford to refuse to answer — "I cannot
order this" and "outside the range" both mean "do not use this one" — and
refusing is honest where guessing is not. The type that answers should be the
range, with both ends, rather than a loose predicate at one end: this ADR is
not implemented, and a floor on its own would admit exactly the newer version
the section above spends its length refusing.

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

No npm on anybody's machine, and no install scripts running there. The
compiled-in digest is kept rather than traded for a lockfile, so the supply
chain story the review of #44 established is unchanged rather than argued
about. Nothing new is hosted. "Unchanged" is doing real work in that sentence
and the next section says what it leaves in place.

The alternative of publishing one archive per agent at release time remains
open and would take the JavaScript out too. It is a bigger change and it adds
artefacts to host; this can be done first and that later, without undoing it.

## What this gives up

Two things, and neither is visible from the paragraph above that says the
supply-chain story is unchanged. Unchanged is the right word, and it includes
what was already weak about it.

**Nothing stops a package install script from running.**
`scripts/desktop/prepare-macos.mjs` runs `npm ci --omit=dev --no-audit
--no-fund`, without `--ignore-scripts`. As it happens nothing runs today: npm
records `hasInstallScript` in the lockfile for any package with a `preinstall`,
`install` or `postinstall`, and the key appears nowhere in either of the two
lockfiles, across 112 packages for Claude and 25 for Codex. But that is a
property of the current dependency set rather than a rule we hold, and moving a
pin is exactly when it would change. A package that gained one would execute on
the build machine, and what it produced would go into the runtime fingerprint
and be notarized as though we had chosen it. Keeping the install at build time
instead of on people's machines is most of why this mechanism was chosen over
the first draft — one machine rather than every user — but the honest
description is that the tree is trusted because it is built somewhere we
control, not because it is measured the way the fetched binaries are.
`--ignore-scripts` costs nothing here and should be passed; if a later change
ever moves an install onto a user's machine it stops being optional.

**A tree in `~/.nessa` is outside the seal that covers the bundle.** Everything
under `Contents/Resources/runtime` is hashed file by file by
`scripts/desktop/runtime-fingerprint.mjs`, checked by `verify-bundle.mjs`, and
then sealed by the application's code signature and its notarization ticket.
Tamper with a bundled agent tree today and the application will not launch.
Nothing equivalent covers what is installed at runtime. The fetched archives
are measured against their pinned digest as they arrive, and `ManagedRuntimes`
afterwards compares every field of the record it wrote — agent, version,
platform, digest, path — and insists the executable open as a regular file
owned by this user and not be empty. That proves *which* release was installed,
and it does not re-measure a byte of it. A copied-out JavaScript tree has less
than that: it has no digest to be measured against at all. So the guarantee
changes shape. Today it is continuous and enforced by the operating system;
afterwards it is a check at the moment of install plus a record, and the window
after that is not covered.

Both are worth accepting. Neither should be discovered later.

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
- `agent_install`'s rationale paragraph and the matching section of
  `docs/codebase-structure.md` have already stopped explaining why one agent is
  special, because none is. What is left for the implementation is the module
  map: the new modules appear and the bundled harness paths go.
- **Three install paths now exist where there was one, and none of them leaves
  an account of what happened.** `ManagedRuntimes` writes a record, but it is a
  record of what *is* installed, not of what was attempted: a download whose
  digest did not match, a version replaced, a partial install rolled back all
  leave nothing behind. #102 asks for an audit port covering started, verified,
  rejected, replaced and rolled back for a single agent's install. This decision
  triples the surface that lacks one, on a path that writes an executable the
  application later launches. The port should land with this, not after it.
- **Running an installer is an effect at the process boundary**, and
  `AGENTS.md` asks for those behind a caller-owned port with a substitute in
  tests. Whatever mechanism is chosen above, the call out to it is injected
  rather than reached for, so a test can exercise a failing install, a partial
  one, and one that reports success and produces nothing.

### The failure modes this creates

Today an agent either works or is absent, and both are settled before anyone
opens the application. Fetching moves that decision to first use, where it can
fail in ways that have to be answered rather than reported.

- **A machine with no network.** Today Claude and Codex work offline because
  they are already there. After this they do not exist until something
  downloads them, and the resolution order runs at every start, so the picker
  is built before any fetch has been tried. Someone on a plane is therefore
  offered three agents and can start none of them, and finds out one at a time.
  Resolution has to distinguish "not installed" from "not installable right
  now" and say which, in the picker, before the pick.
- **What a person sees while 277 MB arrives.** Nothing in this decision says.
  Step 3 offers rather than installs, which is the right shape, but an offer
  needs a size, a progress indication, something that cancels it, and a
  deadline after which a stalled download is reported instead of waited on.
  #121 is the standing example of the alternative: a refusal that reached the
  person as a spinner that never stopped.
- **An install that is interrupted.** The fetch path already answers this and
  the copy path does not. `ManagedRuntimes` unpacks under a temporary name,
  renames, and writes its record last, so a half-finished install is invisible
  rather than launchable, and `installed` re-derives the path from the pin
  instead of trusting what the record says. Copying a 72 MB JavaScript tree out
  of the bundle needs the same discipline — staged and renamed, with the thing
  that says "ready" written after the bytes and not by the copy itself.
  Otherwise a power cut halfway through leaves a directory that looks complete
  and fails at the first `require`.
- **Two Nessas installing at once.** Also already answered for the fetch:
  `ManagedRuntimes` holds a per-agent `install.lock` across publication,
  blocking on purpose because the other install is writing the file this one
  wants. Anything added by this decision joins that lock rather than getting
  its own; two mechanisms serialising separately would serialise nothing.
- **Old versions are never reclaimed.** Superseded artifacts are deliberately
  left where they are, so that moving a pin cannot half-overwrite a runtime
  somebody is running. With one fetched agent that is a rounding error; with
  three, every release that moves a pin leaves a few hundred megabytes in
  `~/.nessa` for good. This decision does not propose a reclamation policy and
  should not, but it is what turns the absence of one from theoretical into
  arithmetic.
- **Linux and Windows, which [#69](https://github.com/nessalabs/nessa-agent/issues/69)
  still has to build.** Three concrete things, all of which are cheaper to
  decide now than to discover there. The pinned set multiplies by platform
  rather than staying at three: Claude publishes eight platform packages and
  Codex six, against Opencode's nine, so the pin file goes from the six entries
  it has today to something near twenty, each with its own digest to measure.
  The build-time `npm ci` is invoked as `execFileSync("npm", …)`, and on
  Windows npm is `npm.cmd` — `execFile` without a shell goes through
  `CreateProcess`, which does not consult `PATHEXT`, so that call fails on a
  Windows build machine until it names the shim or goes through the Node CLI
  directly. And step 2, "already on the machine", needs a per-platform rule for
  where a person's own runtime lives and how its version is read; there is no
  such rule for any platform yet, and macOS's answer will not be Windows's.
