# Worker publication review — closing round 3

Reviewed HEAD `29af2efb974ddb90a95c8921c4192a3603684af8` with the uncommitted changes only to:

- `crates/nessa-server/src/workspaces/infrastructure/release.rs`
- `crates/nessa-server/tests/workspaces/provider_contract.rs`
- `docs/adr/todo/1152-local-vault.md`
- `docs/adr/todo/1160-boat-workspaces.md`

Combined four-file content/path SHA-256: `36aeee5e76b7235a2e95354325fbfe5cd0f59744671ae7f381537b7fe4ad2b4a`.

**Result: no actionable findings at any priority in this scoped delta.** No source, dependency cache, build, credential or remote resource was mutated during review.

## Contracts and relationships checked

Read AGENTS.md and the canonical review/brief/agreement/evidence gates. Inspected the full four-file diff and the actual release capability, `Workspaces::with_image`/`save_token`, composition of that capability, source fingerprint owner and SSH hello refusal.

- The source-derived lease identity remains authoritative. Independently recomputed its Rust FNV-style length-framed/CR-normalized algorithm from the current listed inputs: `066217f7701b98b9`. All listed fingerprinted files are byte-identical to source commit `29af2efb974ddb90a95c8921c4192a3603684af8`. The release mapping/test/documentation change does not alter those inputs.
- `worker_image()` still returns an image only when the compiled source identity equals its recorded artifact identity. Composition consumes that existing capability, and `save_token` refuses `WorkerUnavailable` before saving when it is absent. The new mapping changes no grants, token owner, VM assignment, generation gate or fallback policy. A wrong worker hello is still refused and audited by the existing SSH link owner.
- The new readiness test uses the existing crate-visible infrastructure entry consumed by composition; no private field, test-only constructor, provider mutation or alternate rule is introduced. It catches the concrete integration failure: a valid source build whose shipping mapping silently supplies no worker. It does not prove remote availability or artifact contents merely by asserting Some; those require the separate receipt/clone evidence.
- Source/claim/artifact agreement holds in the supplied evidence: integration source `29af...`, current protocol `066217...`, release share code `bsh_cQ5cYJX64ZJth3NRQsovUo0R8WODUOV_3PXw3shYLuU`, named snapshot, and the new Linux binary digest all correlate. The source commit precedes the release mapping update because that mapping is not part of the leased protocol; it does not purport to be a byte-identical final gateway executable.
- The ADR preserves the old image provenance as historical and documents the new publication. The before-code integration row records the matching-worker invariant. No compatibility alias, acceptance of the old protocol or replacement of an existing VM is introduced.

## Evidence inspected and independent checks

Independently streamed SHA-256 over `/tmp/nessa-1158-worker-nessa`: `e2c9d38b652ee798bca1de09e9b539233aa0c205b79c7880604e9553f885f825`, **137,379,272 bytes**, matching the release receipt. `git diff --check` passes.

Inspected the sanitized release receipt, clean-image, scrub, fresh-clone and cleanup JSON. Clean-image and clone commands each report protocol/checksum success; the targeted credential/build-input checks succeed. Construction separates the source/cache-bearing builder from the runtime-only seed clone. The supplied checks cover named personal credential paths and user SSH directory handling; they are not an exhaustive independent filesystem secret scan.

The before-pin regression log actually runs the new test and fails its shipping-readiness assertion against the old mapping. The positive test and final gateway build were still running when this source review completed; their result is coordinator-owned and must be checked before merge. I did not invoke Cargo or reuse an ancestor's live process handles.

Cleanup JSON records all three owned VM identities absent from the provider resource API. Initial delete acknowledgement records are asynchronous `pending` operations at their capture time. API absence is not a claim that the provider has completed physical data erasure. No such secure-erasure guarantee is inferred here.

## Limits

This fresh review covers this publication/pin/readiness delta, not a new audit of the entire integrated feature stack. Existing integration/security/closing reports remain scoped to their stated source. I did not contact Boat, query account configuration, read any token, provision resources, verify subscription inference, or test existing retained-VM upgrades. A real Boat flow, final positive readiness result, platform/browser checks and hosted CI remain coordinator-owned. The earlier root-provided clone receipts are attributed to the coordinator; independent checks here are fingerprint/file agreement, local checksum and source/diff inspection.


## Deletion evidence closure

After the source review, the coordinator supplied status queries for all three
previously pending deletion operations (`/tmp/nessa-1158-bdop_*-status.json`).
Independently inspected these records: each provider operation now reports
`completed`, with a completion timestamp. This closes the pending-operation
evidence limit above, alongside the resource API-absence checks. It does not
establish a separate physical-erasure or secure-wipe guarantee. Source is
unchanged; the original source/digest review remains applicable.
