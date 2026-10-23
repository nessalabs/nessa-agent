# Typed expiry observer checkpoint

Source `7b83e4255048e0c4ba7516cb5948aa27af730e21`; combined registration source
`0d922ce16543e6627efe8b710ac87a80bd507a96`. This branch is local and unreviewed.
The published registration remains blocked by its separate auth findings.

A lifecycle owner may observe expiry before any delivery exists. A closed typed
observer retains that original fact without inventing a delivery ID. Later
observers cannot replace the committed stop. Existing delivery observations keep
their actual ID. This adds no scheduler, permission decision or product mount.

The four regressions in
[the lifecycle tests](../../../../../../crates/nessa-server/tests/webhooks/infrastructure/outbox/lifecycle.rs)
cover lost publication acknowledgement, cold recovery and retirement, saved audit
actor substitution, canonical decoding, integer overflow, known response and
unknown Sending recovery. [Isolated baseline](baseline.txt): 88 passed.
[Combined canonical server webhook suite](webhooks.txt): 120 passed, zero failed.
[Architecture](architecture.txt) passed. Six focused fail/restore controls were
caught and restored; [results](results.json) and [progress](progress.txt) retain
those observations. They establish enforcement, not independent review.

No live recipient, production data or persistent real key was used. Required
browser/stress/chaos, assembled capability/revoke E2E, fresh permitted independent
review and exact-head CI remain pending. No exactly-once remote-effects claim.
