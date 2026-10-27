# Local review record

Base: `33b0fc6534cc30507061665cbc7d3f04cce48e66` (PR #1166).
Round 1 source: `34672659bb87d44ad9f390e9725830bfe500e7db`.
The state-page update was explicitly dirty during review; evidence files added
during review were excluded from the source review.

## Implementer review

Reviewed the config/credential boundary, scope/authority/effect agreement,
default versus explicit unlimited, shared config publication and caller loss,
audit outcome attribution, provider create/resume, UI stale-result and uncertain
save behavior, and supported platform compilation. Revert probes ignoring the
override or removing snapshot-origin validation failed before the restored
workspace suite passed. The browser's stronger geometry/focus checks exposed
the row-label/action space and WebKit focus problems before the source freeze;
the corrected matrix passed in all four engine/layout combinations.

Full package tests and Clippy passed at the source above. The real provider run
passed and confirmed all four VMs absent. Full frontend/broad browser baseline
failures remain distinct from these feature checks.

## Round 1

Three independent read-only reviewers covered persistence/audit/backup,
domain/API/policy/provider, and UI/lifecycle/verification respectively.

| Finding | Severity | Disposition |
| --- | --- | --- |
| Gateway-wide override map reused one owner's 1000-channel quota | P2 | Removed the misplaced count limit; the shared configuration byte bound remains the owner. Regression accepts 1001 distinct overrides and preserves invalid/duplicate refusal. |
| Authorizing ChannelStore.get(A)'s returned B could later write A | P2 | Refuse a substituted identity before Cedar owner facts are used. Positive B and substituted-A authorization tests distinguish the paths. Connector lookup likewise rejects a substituted channel before provisioning. |
| Membership read-error/dangling refusal paths had no regression | P2 | Added controlled port tests for membership read error, channel read error, missing channel, substituted channel, and positive default/matching membership. Negative cases assert no provider creation or assignment intent. The positive fixture deliberately cannot serve a lease stream, so it proves allocation admission, not successful remote lease setup. |

All three regressions pass on the corrected tree. Each intentional source revert
(restored per-owner cap, removed authority guard, removed membership guard)
failed its regression before the restored tests passed. The corrected source
checkpoint is `e2f32fe2849b88a39a76589f0385acbe7abaf820`; the CI package-selection
all-target Clippy check also passed there.

The UI reviewer reported no findings. Its independent late-save/replaced-gateway
and custom-duration/discarded-draft probes passed; both were then retained in the
normal rendered test suite (6/6 passing). No reviewer edited source or ran Cargo
against the borrowed build cache.

## Round 2

A fresh reviewer inspected the assembled feature and round 1 dispositions at
`e2f32fe2849b88a39a76589f0385acbe7abaf820`. No production defect was reported.
One P3 found that the positive channel-editor fixture omitted channel identity
from its initial read and unlimited save snapshots, despite the real client
refusing such responses. The fixture was corrected in
`471fa51e1f92fad5e17ddd20bc5dee407de1cffb`; client and rendered control suites
passed 12/12 afterward. Ordinary fixtures now carry their requested channel.

## Round 3

A fresh final reviewer inspected base through
`471fa51e1f92fad5e17ddd20bc5dee407de1cffb`: **no findings at any priority**.
The reviewer checked authority/target agreement, membership/snapshot origins,
publication/audit outcomes, provider TTL mapping, client/UI scope and lifecycle,
and the prior dispositions. It independently reran client/rendered suites 12/12
and a disposable client probe accepting four valid combinations and refusing
three contradictory scope/lifetime responses. No source, build, paid-provider
or shared-cache mutation occurred. The original bounded source review loop closed at this checkpoint.
Final verification and documentation outcomes remain the coordinator's work.

## Structural correction after broad verification

The first complete desktop run exposed a row interaction that the initial
settings matrix missed: interactive actions had replaced the count/glyph
identity, so the decorative running glyph intercepted a row click. The correction
removes that conflation: action presence and decoration identity are independent,
with a transparent band and pointer handling restored only on the action wrapper.
The ADR table records the interaction before the correction. The separate CI
failure was a missing canonical generator input in the isolated staging cut.

Implementer checks: production rows 4/4, settings 4/4, repeated full shared-controls
32/32 and smoke 32/32; pointer-rule revert fails count/glyph hit tests, exact source
restored. Bare-Node harness 20/20; actual isolated staging replay 65/65 plus drift.
Typecheck, targeted ESLint/format, actionlint and client/control 12/12 pass.

Fresh post-structural round 4 reviewed base `fd4dbda66e8e6aa4cd1e27fb8d204e2f33a12505`,
head `fbadc7b8de10ab69030fd6a15a584a0c98e09098` and all seven dirty source/test/doc
files. The diff SHA256 is `48f16c0cda9684b22512bab9ed6e0358ebbc6ea443f39ad79a406dd358a0443b`.
It closed clean at every priority. The reviewer inspected count, glyph,
action-only and empty rows, kit sibling layout, click isolation, focus return,
staging consumers and adjacent scope/authority boundaries. It independently ran
20 harness tests and six actual Chromium/WebKit click/focus probes. It made no
source, Cargo, provider or cache mutations. The reviewed source is committed as
`d9cc877` (CI staging) and `29ef4e5985a2858bd0839de5e67050a5078d2a21` (row structure).

Only the affected channel-workspaces state page is advanced under nessa-docs.
The row interaction is simultaneous presentation facts, not a new state machine;
its sources and verification are added to that owning page. No module moved or
new dependency was added, so the existing module map remains accurate. Provider
and backend checkpoint attribution stays at 471fa. Full broad classification and
current-head CI remain separate verification work.

## Final platform review, round 5

Windows CI at fbadc reported an unused `default_override` getter under
`-D warnings`. Its only caller belongs to the Unix-only workspaces module;
portable parser tests use the neighboring getters, not this one. The correction
narrows only this getter to `cfg(unix)`, without suppressing a warning or changing
Unix runtime behavior. The existing Windows Clippy gate is the negative evidence
and the enforcer; local Mac success does not establish a Windows pass.

A fresh read-only reviewer inspected head 29ef plus this one-line source diff
SHA256 `cb079b5f3237644e60160329c163a4f0133dc8b00a6b776ffd313864a55593ea`,
the consumer's outer platform gate, adjacent helper consumers and the actual
Windows failure log. It reported no findings at any priority. The exact local
CI-package all-target Clippy selection passed, and the parser rerun passed 3/3.
The reviewer made no edits or Cargo/cache/provider operations. Final source is
`49080c0477f23e752bb7dd90dc80e093a9d12558`. The affected state-page checkpoint
advances to that exact reviewed source; provider and UI evidence keep their own
source attribution. No other state page advances. Nessa Docs catalog validation
holds for 135 pages with no errors; the installed renderer reports 27 existing
unknown-sourceRevision metadata warnings. Visual renderer interaction was not
rechecked for this prose/source-list-only update.
