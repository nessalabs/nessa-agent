# Principal canonical source independent review, round 1

Reviewed source `ebe4f93e8dae4bbbdda9f571e8db57a03c0a8e59` against published base `d947da064be72322f4a8514bc5017eba865b84a6`, initially clean candidate `a3c04299c0b967f609fcd03b3e97fb8bd3e7b624`. Author subsequently corrected two inherited Markdown deadline-fixture references at clean docs head `6ff699d34bcd33518dd744346f1046804fccd955`; production/test source was unchanged. The author then appended the reviewer-designed replacement-lease test as sole writer for serial execution. Reviewer changed no checkout source, ref, target or cache.

Read AGENTS.md, coding standards gates/review dimensions/agreement/ownership/evidence and bounded rounds, codebase structure, architecture and typed dependency injection. Inspected the complete scoped production diff, all new source-test flows and relevant unchanged admission, Auth/grant fences, native SDK worker execution, original receiver validation, durable queue scope/representation, state pages and evidence claims.

## Findings

- Blocker: none.
- Major: M1 below.
- Minor: none.
- Nit: none.

### M1 — major — a substituted reply lease releases the original global result budget

Location: `crates/nessa-server/src/webhooks/application/source.rs`, original permit handoff at `RecordReadLease::new((permit, authority.clone()))`, returned `RecordReadResponse`, and `WebhookRecords` retained fields.

Reachable trigger: a `PrincipalRecordReadSource` decorator awaits the actual native canonical reader, receives its valid head after physical cleanup, replaces only `response.lease` with `RecordReadLease::new(())`, and returns the otherwise unchanged valid response. The new application validates the data selector successfully and returns `WebhookRecords`. Its independent `authority` Arc still retains the original Auth and grant fences, but the global capacity permit exists only in the source-returned opaque lease. Replacing that lease drops the real permit, so an apparently bounded retained result no longer owns the original global budget.

Expected: the original composition-selected capacity reservation remains held while the bounded result and its original fences are retained, and is released after that result/physical ownership ends. Actual: with a one-slot global budget, the result remains alive and readable while `available_permits()` is already1 rather than0. More than the budgeted number of retained results can therefore be admitted across distinct granted resources. This is an original-resource-accounting defect under a substituted adapter reply, not a demonstrated Auth/Cedar permission bypass: the original authority Arc does preserve credential/grant fencing.

Concrete reproduction: reviewer authored `/tmp/nessa-896-review-replaced-source-lease.rs`; author appended and ran it serially without changing production source. It uses actual synthetic registration/Auth/grants and the actual SDK reader, changes only the returned lease, records permit availability while the valid result is retained, then drops the result and joins/shuts down the real source before asserting. Confirmed **zero passed, one failed**: retained capacity1 versus expected0. Preserved author log `/tmp/nessa-896-principal-review-replaced-lease-before.log`.

The earlier input reservation and a valid returned page cannot be treated as proof that this returned opaque token owns the original budget. Retain the original permit with the original fences in the same shared application/physical read owner, independently of what lease the adapter returns. The receiver API need not change. Add this substitution ordering to the existing source design table, retain the regression, run affected ownership/capacity controls and obtain a fresh full round after correction.

## Independent executed checks

- Current compiled source binary: `webhooks:: --test-threads=4` — **204 passed, zero failed/ignored**,19.74s; `/tmp/nessa-896-principal-review-webhooks.log`.
- `conversation::infrastructure::record_read --test-threads=4` — **12 passed**,0.33s; `/tmp/nessa-896-principal-review-receiver.log`.
- Existing receiver-scope test independently passed; `/tmp/nessa-896-principal-review-scope.log`.
- Independently checked both affected state pages' source paths, relative links and unique headings after the author fixed the inherited deadline fixture links. The record-read page points at exact committed source ebe4; webhook-outbox deliberately preserves its persistence revision for a navigation-only addition. No catalog/render/browser claim.
- Independently compared the pre-controls exported PrincipalRecordScope with final source: its only production difference is the prefixed consumer ID becoming the existing registration UUID. All selector guards are unchanged. Final application source SHA256 `0cee92b10e9c921b5bb034ccb7357d0c98b0a9fd2b26cb7c32815c625644af01` exactly matches the seven application-control originals. This supports the explicitly stated control scope; it does not relabel those ten controls as exact-final-tree mutations. No mutation rerun by reviewer.

Author's seven-package Clippy,108 architecture tests/scanner, formatting and diff results were inspected as supporting evidence; reviewer did not independently rerun compiler gates or invalidate the shared build target. Original binary-framing fixture failure, outbox scope refusal and early compiler/architecture/lint failures remain historical evidence and are not counted as passing gates.

## Agreement and ownership traced

**Authority and identity.** Actual local Auth owns the current credential/membership; the existing admission owner acquires its read hold first, then the exact principal/resource grant gate. It compares target credential/principal/organization/audience, active membership/lifetime, exact conversation/credential/principal/epoch and absence of receiver binding before loading metadata and asking the existing WebhookReader Cedar rule. Source construction occurs only after that current admission. PrincipalRecordScope is selector data and is distinct from ReceiverReadScope; it grants no receiver or product-session authority. Its existing registration UUID consumer ID matches unchanged outbox cursor/stored-row checks. Composition must continue supplying an actual committed registration, not a caller-invented tuple.

**Acquisition graph.** The use case first try-acquires the shared budget without waiting; then unchanged credential -> principal grant -> metadata/Cedar ordering runs. Native worker bookkeeping locks are acquired only after authority admission and are not held while waiting for Auth/grants. Grant changes use the same per-principal/resource gate before their blocking write. Auth revocation waits for the actual admission read fence. The SDK layer does not acquire conversation/Auth locks. The result inspection mutex protects only local shared hold access, is never acquired by the physical worker and crosses no await. No new reversed acquisition was found.

**Cancellation and joins.** Before grant acquisition, cancellation releases the credential read hold and original capacity. After native handoff, the physical closure carries the lease; dropped observers set the existing stop flag while original fences remain with work. SDK source drop joins its inner worker outside Tokio enter; existing ReadWorkers retains OS joins, records unobserved worker panic and keeps shutdown completion across cancelled observers. Tests exercise both normal and panicking held physical work. Source shutdown is physical-work shutdown, not a claim that the still-unmounted command coordinator has drained pre-admission futures.

**Normal result lifetime.** The original authority Arc remains in the result and physical lease, so reply-lease substitution does not fabricate read permission or release Auth/grant fences early. Absolute expiry is rechecked at result inspection; already read data is not itself future send authority. M1 concerns the capacity part that was not included in that same original owner. Retained result lifetime still belongs to the eventual bounded coordinator, not an already implemented supervisor.

**Selection, representation and persistence.** Application validates requested subject/conversation/epoch and owner-selected request limits before source I/O; native identity lookup and exact opened-source checks retain origin/stream/incarnation/schema. Returned head/page selectors must match, and canonical sync validation checks echoed request, record scope/positions and payload bounds. Invalid source paths are tested with no source invocation or unchanged cursor/delivery/audit counts. Valid SDK records use the existing recordsPage wire encoder; raw JSON preserves canonical wire-record bytes, including base64 binary payload. The actual two-record compatibility fixture queues them through unchanged outbox scope and cursor ownership while the source result retains its fences. It is not live runtime source feeding or HTTP delivery.

**Receiver behavior and layering.** Existing receiver signatures, passive admission and original opened-source validator remain unchanged behind the explicit receiver branch. The new shared native branch adds no policy verdict or new SDK/protocol/client contract. Limits are injected into the application rather than imported from product infrastructure. No new persistence schema, migration, credential issuance policy or Cedar rule was added. Source/test/design/state ownership and module maps are coherent within this narrow slice.

## Exclusions and disposition

Round1 is **not clean** while M1 is open. After a scoped correction and affected checks, a fresh reviewer must perform the next full round under the repository's bounded review loop.

No independent supported-platform/MSRV build, exact-head remote CI, browser/catalog/visual rendering or feature stress/chaos qualification. Prior cancellation head d947 CI is historical for this source; the Windows45-minute timeout remains unresolved. No live credential, public listener, production data, remote recipient, CI retry/dispatch or held operation was used.

The source use case is unmounted. Retained registration/ordinary-issue/revoke command ownership, startup/shutdown coordinator, principal share resolution, supervised source feed/sender, known-outcome settlement and complete isolated recipient capability/revoke/expiry/stale-epoch/retry/cancellation/no-send E2E remain unfinished. This review does not complete #896, qualify final full-PR assembly or authorize merge.
