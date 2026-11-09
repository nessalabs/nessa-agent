# Independent round two: shared reset ownership and named persistence owners

Reviewed clean checkout `/workspace/nessa-agent`, branch
`codex/896-webhook-delivery`, at exact head
**546de7af208bdb621bd2bfddc585a7ff10dbac23**. Implementation source checkpoint:
**b3dc0d4688e473fa40bbbeddf00bb9f510130bf1**. All crate source is unchanged between
that source checkpoint and reviewed head.

Bases checked:

- Immediate correction: `43a9094623af1b2aac6b0c3211e1b93573941377`.
- Original immutable reset correction: `0f6bea1e904267e86cd0ec3ee8a858c75371cec1`.
- Intended complete PR1096 stack base: foundation
  `6c38915d5201de4cac54500ec4b5ad5e5f34229e` / PR1091.

**Outcome: clean review of the reviewed persistence/correction scope. No findings
at any priority.** This is not full #896 completion, browser qualification,
exact-head CI acceptance, production registration or merge approval.

## Blocker

Empty.

## Major

Empty. No independent structural constraint was confirmed. The immutable original
reset fact, first-capture membership, context/history correlation and common
reference release continue to address the prior source-provenance finding within
this storage slice. The direct owed selector continues to avoid the optional-OR
published-history scan. These conclusions have the boundaries below.

## Minor

Empty. The first round's organization/import finding is addressed:

- Both `outbox/mod.rs` and `outbox/source_reset/mod.rs` contain documentation,
  module declarations and internal/public re-exports, with no behavior definitions.
- `outbox/store.rs` owns the lifetime lock, bounded physical work, `SqlOutbox`
  implementation and test module declaration. `schema.rs` owns the unchanged
  canonical SQL include. Primitive/current encoding checks live in `codec.rs`.
- Reset/capture persistence lives in `source_reset/store.rs`. `Error`/`ErrorCode`
  are imported at module scope; test-only practical imports also moved to module
  scope. The SQL-read constant is re-exported for tests only.
- Registration's access to the outbox operation/owner clock remains restricted to
  the existing outbox consistency boundary. The move changes internal paths and
  visibility needed by siblings, not the exported authority of `SqlOutbox`.
- The module map and owning state page list the named files. Test source changes
  against the immediate base are import relocation only; no assertions changed.

## Nit

Empty. Independent committed-range `git diff --check BASE..HEAD` returns **0**
with empty stdout/stderr for all three bases listed above at the actual reviewed
head. Results are preserved in `committed-range-checks.json`. Meaningful test log
lines remain; the first round's two EOF warnings and subsequent README EOF are
absent from the final reviewed range.

## Independent execution and evidence

Own directory: `/tmp/independent-webhook-round2`. The harness was copied with all
original probe paths rewritten to this directory. It compiles actual copied
production outbox/domain code and actual SQLite/private-filesystem adapters
against canonical dependency artifacts, substituting only the Target DTO.
No Cargo command, shared artifact invalidation, root source/test edit or Git write
was performed.

- Original copied baseline: **67 passed**, zero failures, **47.55 seconds**;
  `baseline.txt`, `baseline-run.txt`, `compile.txt`.
- Four focused lifecycle/correlation probes: **4 passed**, zero failures,
  **0.05 seconds**; `probes.txt`.
- Final combined probe binary: **71 passed**, zero failures,
  **43.24 seconds**; `all-tests.txt`.
- All **33** copied production webhook source files hash-match the frozen root;
  `source-manifest.json` records SHA-256 values and exact source/head.
- `harness.py`, `add-prior-probes.py`, `add-combined-stop-probe.py`,
  `compile-probes.py`, `args.json` and `probe-args.json` retain reproduction and
  compilation context. The prior three probes are explicitly reused and rerun
  against this moved code; the combined-stop probe is additional in this round.

Concrete checks:

1. **Cursor deletion and later Retry with backward cleanup clock.** After the
   original reset/capture, remove the mutable cursor and trim its published audit.
   Reopen, settle Sending into Retry, and run cleanup with a clock earlier than the
   reset observation. The refusal preserves original reset identity, Live reader
   and time1002 while recording its separate cleanup time500; reopening accepts
   the feasible history. No timestamp ordering is invented.
2. **Cross-chat substitution to an existing valid reset.** Construct two actual
   reset/captured Sending histories, then substitute the first chat's owed refusal
   with the second chat's real reset identity, also updating the SQL FK projection.
   Publication returns Invalid without changing audit files; reopening refuses.
   This tests contextual correlation rather than only malformed or dangling IDs.
3. **Physical source cycle A→B→A→B.** Replace/delete later cursors and create new
   work through a complete source cycle while the first Sending remains owned.
   Restart records Unknown using its first reset membership, not the newest cursor
   reset. Cursor replacement remains a private storage fixture, not a product API.
4. **Distinct source reset and credential revoke through shared cleanup.** Capture
   a Sending reset, then revoke with a different original credential actor/time.
   Its later response produces Retry; credential-stop cleanup refuses it. The
   refusal retains both original contexts, with the credential revoker as its own
   cause/initiator and the original Live reset cause separately. Reopen succeeds;
   draining retirement removes registration, source facts and capture membership.

## Boundaries and relationships inspected

Read AGENTS.md, coding standards gates/local review/dimensions/agreement/audit/
organization/import/DDD/one-current-contract guidance, architecture and typed-DI
guidance, module map, original access-grant webhook contract, parent ordering
matrices, historical source-provenance review, first shared-reset review, current
owning state page and current supplied evidence. Inspected every production file
in the immediate code correction and the full adjacent persistence path:
`store`, `schema`, `codec`, `queue`, `cursor`, `stored`, `source_reset` fact/store/
retention, `audit`, `publication`, `restoration`, `retirement`, `journal`,
`registry`, `capacity`; domain Delivery, application Outbox/causes and admission,
and relevant existing tests/cancellation/retention/restore cases.

- **Authority/identity and policy:** the existing application admission gathers
  current credential, membership, principal/resource grant epoch and conversation
  facts, then asks the existing Cedar path. The moved storage code adds no policy,
  application principal, paired receiver or send authority. Storage registration
  methods still accept trusted facts, not caller-supplied permission decisions.
- **Domain and transition ownership:** immutable Delivery owns legal transition,
  attempt budget, epoch/timestamp pinning and terminal behavior. Audit cause mapping
  delegates to that owner. Source-reset/capture metadata has one persistence owner;
  moved helper/import paths do not introduce another transition or causal owner.
- **Causal facts and restoration:** Fact decode checks canonical typed identity,
  registration/chat, prior checkpoint, source replacement and original reader/time.
  SQL projections are checked. Capture membership correlates original/current
  delivery identity, epoch, position, attempt and timestamp. The history owner
  reaches the original captured state before publishing its reset projection and
  ends at current state. Registration, Cursor and Delivery are exhaustively
  context-checked against retained facts; absent snapshots do not manufacture
  original cause. Historical retirement uses bounded original facts and exact audit
  identity, preserving actors after private registration deletion.
- **Admission/concurrency and cleanup:** one canonical filesystem lock and shared
  owner survive cloned handles and detached blocking operations. Physical capacity
  is retained in the blocking task, with no mutex guard held over an async await.
  Queue/state/cursor/fact/member/audit writes share a short transaction; common
  commit owns retention/retirement. Known response, stop and reset causes remain
  distinct. Sending recovery records Unknown without a POST.
- **Audit/effects and acknowledgement:** the whole owed batch and affected complete
  histories validate before first file publication. Saved bytes and immutable
  existing-file checks remain unchanged. Publication requires one SQL row ack;
  publication/ack failure does not become permission for a duplicate remote effect.
  The additional combined-stop probe checks both causal contexts through cleanup.
- **Representation/resources:** canonical schema validation retains immutable and
  reference-accounting guards. Fact/capture reads enforce the reviewed SQL byte
  admission; factories/startup enforce fact, cursor and retained-row counts.
  Reference counters are checked against indexed real cursor/member/audit owners;
  last-reference deletion, membership cascade and audit trim remain coordinated.
  The direct published-state selector retains its indexed shape. Supplied boundary
  and mutation cases support these inspected rules; this review did not perform
  new native-allocation profiling or arbitrary SQLite-image fuzzing.
- **Public surface/organization/documentation:** named owners and imports now
  follow the explicit rule, including the parent module added in this PR. Public
  `SqlOutbox` facade, schema bytes and test assertions are preserved. The state
  page's sourceRevision is the exact committed implementation source, and no crate
  source changed after that checkpoint. Uncomposed runtime and browser gates remain
  explicit rather than being presented as implemented/verified behavior.

## Supporting checks and exclusions

Inspected supplied source-qualified 97 canonical tests, seven-package all-target
Clippy,108 architecture/checker,27 fail/restore probes, formatting,122-page catalog
and two Chromium151 chart renders. These are supporting evidence; I independently
ran the copied tests/probes and committed-range checks above, not Cargo or browser
scripts. Linux-only copied execution does not certify macOS/Windows behavior.

Canonical gateway Chromium/WebKit, default/raised stress and chaos, and exact-head
hosted CI remain applicable and unqualified by this review. NessaDocs rendering is
documentation evidence only. I performed no CI lookup/retry/dispatch, denied-action
retry, alternate route, live recipient/key/registration, production transmission,
public listener, network/security change, merge or force-push.

Product registration/configuration, registered-principal share resolution,
canonical credential revoke/expiry coordination, authorized source/replay, owner
reset command, supervised sender/live settlement and capability/revoke/expiry/
stale-epoch/cancellation/no-send recipient E2E remain separate required acceptance
work. Integration with main56602fc6 remains uncomposed. This clean persistence
review does not waive those gates or assert full #896 completion.

At completion the root remains clean and at the exact reviewed head. No denied,
pending or uncertain external action occurred in this review.
