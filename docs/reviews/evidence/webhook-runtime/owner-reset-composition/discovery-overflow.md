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
workspace. Final source identity, main integration and affected independent review
remain pending at this checkpoint. No merge or external review
publication is established by this document.
