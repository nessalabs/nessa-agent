# SDK ownership publication evidence — work in progress

Issue #628; draft PR #650. This folder preserves historical source-specific evidence, not a completed merge gate. Final corrected-source gates and independent review are pending.

## Historical source identities

| Proof | Exact source | Valid runtime failures |
| --- | --- | --- |
| Original publication rules | `7993434c6ac27b7e2dc9c9da8e89adfffb0ced04` (`codex/628-proof-publication-799`) | 27 |
| Factory gate and transfer | `cddefd5d00ffc01588b3688a135f4d6a3e35b5c4` (`codex/628-proof-factory-cddef`) | 6 |
| Restored history precision | `141755dde6fc159031047ae23f20c1f14dc6e2fd` (`codex/628-proof-history-141`) | 2 |
| Atomic uncertain-root seal | Corrected dirty source on parent `bb24a364`, committed as `1bf09a5ef83f2a4183f3dfbc05e057d5e0138a31`; manifest records restored source hashes | 2 |

Invalid compilation attempts are retained and excluded. The 37 failures above are historical runtime proof, not a claim that every probe ran on the final candidate. Before/after rebase SDK byte equality is recorded in `628-restack-evidence.json`.

Preserved gate directories identify their own source. Completed isolated SDK and six-package tests at `1bf09a5e` passed, but CI run `37622329149` failed the required domain coverage gate: graph.rs has three uncovered regions and one uncovered line. Its excerpt is retained. This candidate is not approved.

## Structural self-review correction underway

One admitted-spawn typed-error boundary will revoke graph/shared-gate runnable authority before fallible fallback publication or unused capacity return. Only the child created by this invocation belongs to that boundary; lookup/conflict/pre-admission failures cannot close another transaction's child. Actual cleanup owners, receipts and the first close cause remain factual. No physical absence or completed closure is inferred from error or Ended progress.

Closing vacant cleanup binding remains valid until actual Released, absence or Closed evidence refuses it. The legacy Open+Ended{Reserved} guard remains narrow. Historical factory completed-startup refusal evidence must be read against its original source; the current runtime graph-close correction changes that case and needs new regression evidence.

Panic supervision #625, mandatory cleanup publication debt #646, failed-startup parent settlement #649, and blocking adapters #627 remain separate. Final exact-source gates, supported-platform CI, both independent reviews and dispositions will be added before merge.
