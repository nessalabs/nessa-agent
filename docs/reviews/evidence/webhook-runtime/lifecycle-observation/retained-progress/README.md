# Retained-progress structural correction

Parent adjudication adopts A after [round-three M3](../review-3/report.md).
The design table is in [the implementation record](../../../../../design/webhook-delivery-implementation.md#retained-initial-publication-proof-parent-decision-a).
This is an ephemeral summary over retained facts, shared by cold restoration
and live publication. It is not new durable history. Erased proof cannot establish
historical acknowledgement. The existing file-written/ack-lost case remains valid.

`restoration/progress.rs` consumes the existing audit decoder and contextual
validator, reads each affected registration's retained audit once through a new
registration-leading index, rejects competing initial identities, and refuses an
owed initial record if current lifecycle or retained work contradicts it.
The schema inventory covers all eight tables; subordinate members derive ownership
through reset/delivery references, while original intents establish input binding.
Exhaustive matches require a decision for new Record or Cause variants.

The two M3 regressions require Invalid before any live publication or cold effects;
repair removes only the injected contradictory row and valid recovery resumes.
Additional controls cover two pending/published identities, proof erasure, first
expiry/retirement/current-work families, unowned reset substitution, unrelated
registration work, exact file-written/ack-lost recovery and bounded indexed reads
at the unchanged 1,024 retained-row limit. Query metadata probes do not claim
restorable application or authorization history.

The first canonical attempt caught a new retired-owner error ordering mistake
(Unavailable instead of Invalid); WORK is now checked before requiring a live
owner. The first private run caught an incorrect test expectation: normal cleanup
trims the acknowledged SQL projection. Both original failures are preserved here;
the corrected control checks no owed row and the unchanged original file.

Source checkpoint: **e08b90cf01ecd2fef3e78ce4b707935e1fb2abfa**.
Canonical webhooks: **160 passed**, including unchanged full-boundary retention.
Seven-package all-target Clippy with `-D warnings`, formatting and architecture
checks passed. The existing lease-protocol long-constant-evaluation warning
remains unrelated. Eight mutation controls caught the changed rule and passed
after byte restoration. The private pre-final suite passed129; the final focused
suite passed11. Both M3 tests fail against the actual old 4c993d45 owner and pass
with the correction. The docs catalog has135 documents and no issues. All three
diagrams rendered at the exact checkpoint in system Chromium151 with no page
errors. Visual inspection simplified overlapping reset self-loop labels; the
new initial-proof refusal arrow is visible. This render is documentation only.
[Fresh independent structural review](review-1/report.md) is clean at
**e8f1564f90e34f77bee6858328f34d94a323c01e**: all priority buckets zero,
214 actual Auth tests and133 combined owner/probe tests passed, with four
independent mutations caught/restored. Production copied files remain byte-equal.
This qualifies the inner W24 correction, not the full main-based feature. W24 remains unpublished. Product command
ownership, general revocation mapping, source/sender composition and capability/
revoke recipient E2E remain separate required implementation work. Canonical
scripted Chromium/WebKit and platform CI remain required feature gates; a local
system-Chromium documentation render does not qualify them. Prior denied actions
remain stopped, including pinned browser download and foundation CI retry.

Published predecessor c5 exact-head CI40024786910 has a Windows local-auth failure
in native enrollment: actual peer_audit_unavailable versus expected peer_unreachable
at tests/peer_gateways/enroll.rs:535. All7revocation tests pass on Windows in that
job. The failing assertion and peer audit/command production owner are unchanged;
fixture DTO purpose and session Cedar injection were mapped by the earlier auth
correction. This is not proof of an unrelated baseline flake. The failure remains
unqualified; no retry, timeout change or unrelated repair was applied. The excerpt
is preserved here. Windows server and other platform jobs were still running at
the last read. Those old-head results do not qualify this W24 source.
