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
or shared-cache mutation occurred. The bounded source review loop is complete.
Final verification and documentation outcomes remain the coordinator's work.
