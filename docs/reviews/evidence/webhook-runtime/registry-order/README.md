# Ordinary registry writer handoff

Production source: `d99a7520532a86406f51853b37d8e9f62c71a0fe`.
Corrected regression source: `9bef5b428a6b987136544109f5bf8e286e29297f`.
Clean independent review: `da2146c0ee1ef731c4740284b330c26b9f26a956`.
This candidate preserves published `af9ea378` and main `d3dbf580`; the unpublished
product runtime is excluded. This slice does not complete outbound delivery.

Historical exact-head CI failed the native enrollment test
`peer_enroll::a_refused_or_wrong_class_invitation_saves_nothing` at
`tests/peer_gateways/enroll.rs:685`: `Owner(Enrollment(Unavailable))`.
Ubuntu run `40192644432`, job `121166805867`, and earlier macOS run
`40187992089`, job `121153452531`, retain that original outcome. The subsequent
listener `RecvError` is teardown evidence. Their exact holder/cancellation site
was not observed. No timeout, retry or assertion was relaxed.

A synthetic real no-op expiry held inside its Clock reproduced the mechanism:

```text
holder=expiry-clock entered=Ok(()) held=true raw=Some(Locked) owner-expiry=Err(Unavailable) owner-decision=Err(Unavailable)
holder=expiry-clock exited=true released-expiry=ok released-cancel=terminal
```

The new regression failed on unchanged production source: 0 passed/1 failed,
with an early cancellation `Err(Unavailable)`. On corrected source it passes:

```text
holder=expiry-clock entered=Ok(()) started=[Ok("expiry"), Ok("decision")] held=true early=Err(Timeout)
holder=expiry-clock exited=true serialized-expiry=ok serialized-cancel=terminal
```

The timeout above is the unchanged two-second negative-observation watchdog,
not an operation timeout/retry. Assertions check both distinct successful worker
entries and the original holder before explicit release, then actual results
and the single canonical cancellation revision after every worker joins.

Author Linux checks: 215 Auth library tests; strengthened regression 1/1;
Auth all-target Clippy `-D warnings`; 108 architecture tests plus architecture
check. Existing actual effect-hold/revocation and registry-lifetime controls pass.
The exact clean candidate also passed the named native enrollment test 1/1.
The original reproduction, failed regression and all raw check logs are preserved
in the execution workspace; neither a local pass nor the reproduction identifies
the historical CI holder or qualifies Windows/macOS.

[Round one](review-1.md) found one minor test-evidence gap; the production lock
analysis found no defect. [Fresh round two](review-2.md) is clean at all priorities.
Only technical evidence/docs are added after that frozen candidate; production
and test bytes remain unchanged. Exact published-head CI/platform and final
assembled recipient/lifecycle/browser/stress qualification remain pending.
