# Registration correction checkpoint

Implementation source `6cfb9673aba7c9fb78e26f4e3c09f2dd55be279f`; intended base
`501a81dca92d0af7f7d8fa457bd1ec409cdf3b89`. [Fresh round2](../review-2/README.md) is incomplete: new origin/evidence findings remain open, and final report delivery failed. The correction checks below do not qualify the full feature.

[Round1](../review-1/report.md) remains a not-clean historical report at7041ac65.
Its passing demonstration tests show defects; they are not fixed-test passes.

- MajorR1: `RegistrationIntent.resolve` is the one command-correlation owner.
  The application resolves returned selections against its original call before
  auth; SQL replay invokes the same owner. Full caller request, destination and
  key remain original; only selected IDs and original observation/time can differ.
  Installation's SQL identity lookup plus exact retained-value comparison owns
  its separate relationship. The added substituted-adapter test changes17 fields
  before or after reserve (31 concrete cases), including malformed returned
  selections; no credential or registration may be created.
- MinorR2: row stepping now uses the existing typed row-error mapper. A test-only
  SQLite progress handler interrupts valid scans; each interruption is Unavailable,
  and the exact unchanged rows validate once the callback is cleared.
- [116 canonical webhook tests](webhooks.txt), [seven-package all-target Clippy
  with -D warnings](clippy.txt), [architecture](architecture.txt), and formatting
  passed. Auth source is unchanged from the prior212-test run. No broader source
  or browser qualification is inferred.
- [12 failure/restore probes](probe-results.json), including each resolver
  consumer, selected identity/time projection, caller/config/key binding and
  transient row-error mapping, were caught; every restored control passed.
  [Copied source hashes](copied-source.json) match the frozen source exactly.

Docs validated135pages/0issues. Four diagrams rendered without page errors at
this source checkpoint; the changed original-reservation guard was visually
inspected. SystemChromium151 documentation rendering does not qualify canonical
gateway Chromium/WebKit.

KnownK1 purpose/kind and no-product-session proof remains open. No real recipient,
credential, live registration, listener, production action or denied CI retry was
used. Product/W1/source/sender and actual recipient E2E remain unfinished.
