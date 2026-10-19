# Offline restore planning (#270, partial)

The proposed `nessa-backup-plan --bundle ABSOLUTE_PATH --mac-key-file ABSOLUTE_PATH`
command authenticates and verifies the supplied cut with the existing verifier,
then writes a JSON planning report to stdout. It accepts no destination,
activation, identity, policy override or executable-plan input. It creates no
staging directory, journal, profile, receipt or imported records. Exit 0 means
an inspection report was produced, never that restoration is possible or allowed.
The report is not an authorization token, reservation or replayable plan.

The command reuses the standalone verifier merged in #971. Composition supplies
the infrastructure `ExportFormat` through the application's `VerificationFormat`
port for both the manifest and deletion inventory. There is no second persisted
decoder or format fallback. It adds no private-storage changes and does
not implement or fix #324 Windows retained-directory replacement. Its filesystem
observations inherit the verifier's current retained-root checks; changes from
#324 must be integrated and retested before relying on their additional guarantee.

## Ordering and refusal table

| Row | Input / event | Observation or result |
| --- | --- | --- |
| P1 | Unauthenticated, malformed, unsupported, unsafe, incomplete public layout or over-budget cut | Return the existing verifier failure; no plan is emitted. No second parser or filesystem enumeration. |
| P2 | Current-format authenticated cut, including an empty cut | Produce an inspection report with activation unavailable. Authentication uses only the supplied key; key provenance/recovery and off-device freshness remain unresolved. |
| P3 | Public payload fingerprints match | Format integrity is verified; every store payload still needs its owner's schema/semantic compatibility checks. No database or provider context is opened. |
| P4 | Authenticated redacted/skipped/deferred omissions, including repeated labels | Preserve all verifier rows; summarize counts by writer-owned store inventory without inferring unique source origins or treating public overlap as recovered data. Deferred bytes are not fetched. |
| P5 | Inventory absent, or authenticated inventory present | Report absence versus verified format separately. Neither establishes latest revocations or an independent rollback anchor. |
| P6 | Proposed import sequence | New private destination selection, persisted quarantine before consumers, store-owner validation/import, independently trusted inventory reconciliation, identity/fencing proof, then separately implemented audited authority acceptance. Every step is unimplemented; none is executed. |
| P7 | Any cut, even one with zero omissions | Trusted-key recovery, freshness authority, replacement identity, source/executor fencing, omission acceptance, payload compatibility, quarantine/import/acceptance implementation remain unresolved. No flag bypasses these prerequisites. |
| P8 | Actual standalone process, including hostile historical command/outbox/lease/pairing bytes and failure paths | No profile/input writes, listener, connection, pairing, lease issuance, outbox delivery, command/tool execution or background process. Synthetic canaries and Linux network/process syscall trace verify this boundary. |
| P9 | Injected decoder refuses manifest or authenticated inventory | Forward the same format port to the verifier and return its typed refusal without a plan. Manifest refusal precedes MAC/payload access; inventory refusal follows authenticated, fingerprint-checked bytes. No alternate decoder or import path is used. |

Omission summaries describe supplied metadata only. An omitted label is not a
unique restore key, a schema decision, or a choice of which missing data can be
accepted. Store payloads are opaque. The authenticated manifest is the format
compatibility evidence; SQLite schemas, cross-record histories, provider restore
support and off-device authority remain unknown. The supplied key's success does
not prove it will survive loss of the source.

The report lists unresolved prerequisites without selecting a key-recovery
backend, identity recovery/re-pair policy, freshness service, fencing policy or
retention backend. #854 exact-version payload retention/fetch stays deferred.
A future importer must reverify its own bytes; this inspection acquires no lease
on the cut and cannot safely be used as later activation evidence.
