# Discovery overflow correction

Codex finding [4374091994](https://github.com/nessalabs/nessa-agent/pull/1101#discussion_r4374091994)
applies to published `1bf581ef7c64abe2d333b0d7eb53730071f9644a`.
It is an open correctness finding until the affected implementation and tests
have independent review. Earlier whole-feature rounds and closures are retained;
this correction does not restart that review counter.

## Cause and bounded contract

The share owner limits grants per conversation. It legitimately permits one
principal to have more than 1,024 conversations. Discovery previously reported
its scheduling bound as malformed metadata. The runtime converted that result
into permanent supervisor failure, also sealing shared command admission.

| Owner / input | Result and effects | Next opportunity |
| --- | --- | --- |
| Conversation metadata / at most 1,024 validated matches | Return the complete bounded candidate set; each physical read still requires fresh canonical admission | Existing capture and send paths |
| Conversation metadata / 1,025 validated matches | Return Capacity, with no partial candidate set | A later discovery pass reads current grants |
| Runtime / discovery Capacity | Record existing process-local source-unavailable refusal; skip this registration's capture pass, retain shared command admission and continue siblings | Existing paced cycle, or wake after a control command |
| Runtime / malformed discovery metadata | Fatal supervisor refusal; no new capture from that result | Existing failure and owned cleanup contract |

The query still selects at most limit plus one rows and validates returned IDs
and revisions before classifying overflow. It does not silently truncate,
increase the limit, reject otherwise valid owner grants, or create a new policy.
No Auth/grant hold, native connection or delivery pin spans discovery retry.
Retained due work keeps its existing fresh admission checks; discovery overflow
does not reinterpret its payloads or promise to suppress every previously queued
send. The refusal itself performs no native queue, cursor, audit or POST effect.
The existing last-refusal diagnostic is an observation, not current authority.

## Evidence and limits

On October 26, the initial unchanged-production reproduction completed with one
failure after 13.19 seconds: a healthy sibling received no POST, native mutations
reported Stopped, unshare returned conversation_storage_unavailable, revoke
returned credential_store_unavailable and cleanup returned Unavailable. Its
original `/tmp` log did not survive the saved-environment resume; these numbers
are historical task receipts, not a recovered artifact.

The recovered test creates 1,025 distinct conversation records and commits their
grants through the authenticated public owner share API. The restored-source
October 28 run passed all 11 selected discovery tests, including actual loopback
TLS sibling delivery, zero overflowing-registration capture, public excess-grant
removal, same-runtime delivery at exactly 1,024 matches, subsequent revoke,
healthy retry and owned shutdown. A malformed-metadata control confirms fatal
refusal with no delivery rows or recipient request. This local result does not
qualify a later main integration or replace exact-head platform CI.

Two verified counterfactuals failed as intended: restoring Metadata for overflow
blocked the sibling and shared commands (12.90 seconds); swallowing malformed
metadata failed the supervisor-stop assertion. Both production files were restored
byte-for-byte with fresh modification times. Fresh logs are retained in the saved
workspace. Main was integrated at `4b33de21f7fd68df83e308de4134ef1dda82f01f`,
preserving `1558188dd489aad18fb1807134340092ec107d3a`. The combined webhook
selection passed 297/297, including all retention and recipient cases. The merged
native-session lifecycle control passed 1/1.

Independent affected review found no implementation defect, with one minor
evidence request: individually break registration-local progression. The verified
`continue` to `return Ok(())` mutation failed the sibling regression after 13.05
seconds while commands, unshare, revoke and cleanup still succeeded. Runtime
bytes were restored to SHA256
`3b0e938590a069bbad0b148dc8e7033dd3ec24832eaf71366f245b33125641c4`.
The positive fixture now also asserts zero cursor rows before recovery.
The final restored selection at `0d98a91d287a99096192a2ff941dd2b88e0572a3`
passed 297/297 with zero failures or ignored tests in20.31seconds.
[Independent E1 closure](discovery-review.md) leaves no findings in the bounded
affected review. Required locked eight-package all-target Clippy with `-D warnings` passed
in2m01s against the restored source. New exact-head CI remains pending. No merge
or submitted GitHub approval is established by this document.

The restored-environment frontend run passed formatting, lint, types, architecture
and protocol controls, then reported 5,467 Vitest passes and two private-file
failures. Both failing test files and their adapter match upstream main. This
user namespace reports `/` owner UID65534 instead of trusted root UID0 or current
UID1000; an isolated synthetic probe confirmed the refusal at that acquisition
ancestor. No privacy check or namespace/security setting was changed. The separate
production build passed in15.61seconds. Full frontend acceptance is pending the
normal exact-head CI host; this local gate is not recorded as green.
