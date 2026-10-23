# Registration checkpoint

Source: `460a1c85935973f367cfd5fd033bcb81dd181469`. Base: `501a81dca92d0af7f7d8fa457bd1ec409cdf3b89`.
This checkpoint adds bounded private intents and the inner registration use case;
it does not compose a product route, W1 supervisor, source reader or sender.
[Independent review round1](review-1/report.md) is **not clean** at
`7041ac656528c88776ebb0931500a8667f52a1df`: MajorR1 is reserve-output original
configuration/request correlation; MinorR2 is row-step interruption wrongly
classified Invalid. Both are corrected at `6cfb9673aba7c9fb78e26f4e3c09f2dd55be279f`, with [fresh review pending](repair/README.md). The known immutable
purpose/kind gap K1 is separately reproduced; no send authority was demonstrated.
The 84-test independent baseline and three passing defect demonstrations are
historical evidence, not fixed regression passes.
The [render result](render.json) records four rendered diagrams with no browser
page errors; all four screenshots were visually inspected. This uses system
Chromium151 for documentation only, not canonical gateway/browser qualification. Earlier auth issuance review remains
preserved in [issuance](../issuance/README.md).

- [Seven-package all-target Clippy with `-D warnings`](clippy.txt),
  [server all-target check](check.txt), and [135-page docs validation](docs.txt) passed.
- [114 canonical webhook tests](webhooks.txt), [212 auth tests](auth.txt), and
  [architecture](architecture.txt) passed on the registration source.
- [21 failure/restore probes](probe-results.json) were all caught and all restored
  controls passed; copied production source was byte-identical afterward. The
  isolated harness uses the actual copied outbox/domain/application modules and
  current compiled auth owner. It does not replace the canonical suite.
- [Initial probes](initial-probe-results.json) exposed three coverage gaps, now
  closed by coherent substituted-command, inactive-membership, and altered
  installation-input cases. Earlier uncaught results remain evidence.
- [Schema defect demonstration](schema-defect-before.txt) showed acceptance of
  missing guards at `c0ef8c35`. It is not a fixed-test pass. The schema owner now
  validates every non-internal definition before restoration; the final suite
  covers missing guards, weakened references and an extra effectful trigger.
- Earlier [reference fixture](earlier-reference-fixture.txt) and
  [capacity/decode fixtures](earlier-capacity-and-decode-fixtures.txt) failed as
  new FK invariants took effect or exposed an incorrect typed decoding result.
  Fixtures now preserve live FK refusals before modeling offline corruption;
  capacity fixtures construct matching intent facts, and decoding uses the
  existing typed row-error owner. Assertions were not weakened.
- The [empty-grants fixture](empty-grants-invalid-fixture.txt) was rejected by
  auth before it could test original-command binding. The corrected fixture
  issues a valid substituted grant list and is now load-bearing under probe3.
- Two architecture setup failures are retained: [missing Cargo](architecture-missing-path.txt)
  and [unset writable Rustup home](architecture-missing-home.txt). The normal
  configured toolchain invocation passed; no permission escalation was used.

All recipients and keys are isolated synthetic fixtures. These tests make no
webhook HTTP request. Product capability/revoke/expiry/cancellation/cleanup E2E,
canonical Chromium/WebKit, default and raised stress/chaos, exact-head CI and
final assembled review remain required. No denied CI action was retried.
