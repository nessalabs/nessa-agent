# Parent adjudication: source-reset ownership alternatives

Frozen review head5bb457dba2048f03de91fdc67162636b549437b2;
sourcefb0f403648a2bb9d8dc6f43385c1e1e5371588f2. Fresh structural review
reproduces source-stop actor/time substitutions both on Sending capture and
Queued source-change refusal while retained cursor.reset_stop keeps the original.
This is a persisted-evidence integrity gap, not a demonstrated send bypass.

A (recommended): immutable SourceReset fact with stable identity, reg/chat,
complete prior checkpoint/source, observed source and original reader/time.
The source-replacement transaction creates it once; current cursor and each
actually affected delivery reference it. Delivery history evidence retains that
identity, and restoration resolves/correlates it against the original fact.
An admitted row preserves its FIRST reset identity; later reset events cannot
replace that cause. A cursor may advance/delete after its own audit is published,
while the reset fact survives as long as any cursor/delivery/owed audit depends.
Common journal cleanup removes a reset fact only after all such owners release
it; original immutable audit file has its existing independent retention policy.
Define per-owner/global fact limits in terms of bounded retained cursors and
pending/terminal deliveries, then reject excess before bulk reads. No new
permission engine or POST phase, no current-owner reconstruction. This replaces
value-copy provenance with one owner and keeps existing in-flight settlement.

B: model SourceCaptured as an explicit complete aggregate transition/evidence,
with the reset identity/original context in both its before/after evidence and
current aggregate. Eliminate the Sending last-audit-None exception. Still retain
original reset identity while affected histories depend; otherwise joint audit
and delivery substitution remains undetectable after cursor deletion. More
state/transition changes and audit volume, but each capture is explicit.

Not sufficient: join only against CURRENT cursor.reset_stop, because a valid
later owner reset/deleted cursor must not erase an earlier Sending observation's
original source-change cause. Also not sufficient: append another audit snapshot
without retaining/correlating the original reset identity. Do not relabel stop
with restart/current owner or infer ordering from wall-clock timestamps.

Required matrix before implementation: reset with queued/retry/Sending; stop
before/after observation; multiple resets with one old Sending; same and later
source identity; cursor reset/delete before response/restart; missing/malformed/
foreign reset refs and substituted actor/time; lost SQL/publication ack; prune
published histories with live pending refs; fact capacity/retained workers and
exact acknowledgement. Convert both new demonstrations into Invalid-before-
publication regressions. Keep existing positive later-cursor-deletion and
Sending-capture/recovery controls. Review under parent-approved bound; preserve
all prior reports and source-qualified tests. No runtime composition until clean.


## Parent decision after this report

The parent adopted alternative A: one immutable retained reset fact with stable
identity referenced by cursor and actually affected deliveries, bounded reference
retention/cleanup and transactional capture. Implement after preserving this
report. Mutation/recovery evidence must include later cursor replacement/deletion,
registration retirement, logical access-epoch differences, identity/actor/time/
scope/dangling references, and count/byte capacity before partial effects.
The report identifies no other independent structural finding: invented capture
is the same provenance gap; the minor selection cost is a direct SQL-shape fix.
No comparison only to the current cursor and no extra exception states are
adopted. Composition still requires clean persistence review; no denied CI retry
or weakened security gate is authorized.
