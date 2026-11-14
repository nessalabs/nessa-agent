---
id: feature-refresh-1268
title: Placement and access refresh evidence
kind: guide
status: implemented
summary: Scoped source review of pool order, channel movement and remembered app revocation.
parent: nessa
sourceRevision: "31d023ed3e4cd60053f771d44137f6af1e1b92ca"
diagramLinks: {}
sources:
  - crates/nessa-conversation/src/application/placement.rs
  - crates/nessa-conversation/src/application/service/channels.rs
  - crates/nessa-conversation/src/application/service/app_calls.rs
---

# Placement and access refresh evidence

This refresh adds source-grounded task guides for three newer feature families at `31d023ed3e4cd60053f771d44137f6af1e1b92ca`:

- [Pool preferences](services/desktop/workspace/choose-worker-pool-order.md).
- [Channel run location](services/desktop/workspace/change-channel-run-location.md).
- [Remembered app access](services/desktop/workspace/revoke-remembered-app-access.md).

They do not carry models or fresh formal-verification receipts. Existing polling, permission-rule and workspace-allocation models cover different slices. All unrelated sourceRevision/modelCheck records remain intact. The earlier SDK context/recovery, queued native submissions and channel-backup inventory updates already have their scoped atlas records; they are not newly requalified here.

## Verification status

The canonical catalog and coverage checker accepted 177 pages with no issues. The state-docs wrapper/coverage tests passed eight checks; one opt-in TLC integration test was skipped because this change adds no models. The four new pages rendered three diagrams in headless Chrome with no render or page errors. These checks validate the documentation tooling and pages, not application behavior.

The linked Rust and Vitest product regressions were inspected as source but were **not run** for this prose-only refresh. This isolated checkout has no matching frontend dependency installation, and no whole-server build was undertaken merely to refresh prose. Real cloud, native desktop, Windows/Linux and conversation stress were not run. Existing historical receipts retain their own dates and scope.

Evidence files are [catalog](../../verification/state-models/feature-refresh-1268/catalog.json), [mechanical tests](../../verification/state-models/feature-refresh-1268/mechanical-tests.txt) and [changed-page browser results](../../verification/state-models/feature-refresh-1268/browser.json). [Independent reader/source review](../../verification/state-models/feature-refresh-1268/independent-review.md) closed the terminology and navigation findings with no unresolved material issue in scope. The source-only formal gaps remain explicit even when documentation checks pass.
