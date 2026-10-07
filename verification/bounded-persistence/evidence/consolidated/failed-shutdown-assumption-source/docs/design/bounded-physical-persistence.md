# Bounded physical persistence (#627)

Scope: SqliteOwnershipStore, FileRecords (all five combined-port methods), and
FileAuthorizationAudit. Constructors and disk formats stay unchanged.

```text
async caller -> await instance Worker::admit -> clone input -> Admission::submit
                                                             (synchronous handoff)
                                                job owns input/state then permit
caller drop ----------------------------------> job continues through physical I/O
                                                and owned cleanup -> release slot
next same-instance caller -> waits ------------------------------------^ 
```

One implementation in `nessa-local-storage::physical_operation` owns physical
admission and captured-cleanup lifetime. Each adapter owns an independent Worker
instance and maps interruption into its existing typed port outcome. The optional
`physical-operation` feature is default-off; synchronous consumers do not acquire
a Tokio normal dependency.

Admission captures the originating Tokio Handle before waiting for its instance
slot. Missing runtime context is typed pre-effect unavailability. A queued Send
port future can resume on an ordinary thread using that captured executor. The
originating runtime must remain alive for reliable completion; shutdown is a
typed conservative worker failure, not runtime-independent persistence.
Admission precedes large clones and blocking submission. The owned job holds its
permit through operation, input, state and local guard destruction. This is an
instance boundary, not a global/path lock or a shutdown-drain API. SQLite read
returns empty only for NoRows. Interrupted write returns Uncertain; interrupted
secret publication/deletion returns Unknown; read interruption is unavailable.
Audit retains its two writes: interruption can leave a partial line.
Coordinator revision and settlement authority remain in application/domain code.

| # | Ordering | Required result | Regression witness |
|---|---|---|---|
| 1 | Current-thread runtime; ownership write held in actual adapter before SQLite effect | Independent heartbeat/timer advances while write remains held. | ownership_write_keeps_current_thread_heartbeat_running |
| 2 | Same for ownership read held at query/decode | Heartbeat advances; read result remains pending then validates real snapshot. | ownership_read_keeps_current_thread_heartbeat_running |
| 3 | FileRecords nonsecret load/store held at actual file helper | Heartbeat advances; actual JSON/durability path resumes and completes. | record_store_keeps_current_thread_heartbeat_running / record_load_keeps_current_thread_heartbeat_running |
| 4 | FileRecords secret load/store/delete held at physical helper | Heartbeat advances; actual sealing/file/delete operation resumes with existing typed result. | secret_store_keeps_current_thread_heartbeat_running / secret_load_keeps_current_thread_heartbeat_running / secret_delete_keeps_current_thread_heartbeat_running |
| 5 | FileAuthorizationAudit held at actual append/sync helper | Heartbeat advances; real secret-free record appended only after gate release. | authorization_audit_keeps_current_thread_heartbeat_running |
| 6 | A holds instance worker; B/C wait async admission | Only A submitted/started; no growing blocking jobs; heartbeat continues. | admission_precedes_owned_input_transfer / canceled_sqlite_write_excludes_following_io |
| 7 | B canceled while waiting admission | B creates no worker/effect; A completes; next waiter can admit. | canceled_sqlite_write_excludes_following_io / canceled_secret_store_excludes_delete_and_load |
| 8 | A admitted and queued in blocking pool; A caller drops before closure starts | A still owns slot; B cannot start; A executes once when pool permits. | queued_canceled_worker_retains_slot |
| 9 | A executing actual SQLite write; caller drops; B write/read arrives | Permit remains held; B starts only after A physical completion; final snapshot order follows existing coordinator fence. | canceled_sqlite_write_excludes_following_io |
|10 | A executing secret write; caller drops; delete/load/replacement arrives | All same-instance operations excluded until A ends; no late old publication after the later delete/replacement. | canceled_secret_store_excludes_delete_and_load |
|11 | A executing delete; caller drops; replacement secret write arrives | Replacement waits for actual delete completion, then publishes; old delete cannot later remove replacement. | canceled_delete_excludes_replacement |
|12 | Audit A caller drops mid append; audit B arrives | B waits until A physical append/sync ends; successful appends retain existing order and secret-free fields; interruption may leave partial bytes and is reported as failure. | canceled_audit_excludes_next_append |
|13 | Worker panics before effect | Typed conservative method outcome; permit eventually available; no fake absence/acknowledgement. | pre_effect_worker_panic_is_typed_and_landed_effect_is_uncertain / interrupted_record_operations_are_unavailable_or_unknown / interrupted_audit_and_poison_are_failures |
|14 | Worker executes write/publish/delete/append then panics | Ownership Uncertain; secret Unknown; delete Unknown; audit failure; recovery/load observes actual disk outcome. | pre_effect_worker_panic_is_typed_and_landed_effect_is_uncertain / interrupted_record_operations_are_unavailable_or_unknown / interrupted_audit_and_poison_are_failures |
|15 | Disk/read/decode/lock-poison failure | Correct typed failure; no empty snapshot/None invented; no runtime-thread lock wait. | storage_read_decode_and_poison_are_typed |
|16 | Ownership newer revision publishes before older snapshot reaches coordinator fence | Existing fence skips old copy; worker adapter does not bypass/duplicate fence. | existing coordinator::lifetime_races::an_older_snapshot_does_not_replace_a_newer_seal |
|17 | Adapter instance A held; independent instance B/directory operation | B can progress; no new global lock/serialization guarantee. | independent_instances_progress |
|18 | Panic/caller disappearance while worker state/input cleanup is still physical work | Permit covers worker completion; later operation never starts merely because its original waiter exited. | worker_input_drop_keeps_slot |
|19 | Port future first polled without a Tokio context | Typed pre-effect unavailability; constructor does not need a runtime; no panic/effect/worker submission. | no_runtime_first_poll_is_typed |
|20 | Send port future starts waiting for admission in Tokio and resumes on an ordinary thread | Admission carries its originating executor; live originating runtime executes the physical job and caller observes actual result. | queued_send_future_resumes_on_plain_thread |
|21 | Originating executor shuts down after admission began | No promise of runtime-independent completion; failed join stays typed conservative unavailability/uncertainty. | originating_runtime_shutdown_is_typed |
|22 | Caller disappears before physical operation panics with a faulting panic payload | Physical Job catches call panic while still owning capture; forgets payload without inspection; captured cleanup remains under permit even without caller. | detached_caller_faulting_payload_is_contained / central detached_call_fault_payload_is_not_dropped |
|23 | Queued blocking task is discarded before starting by abort or origin-runtime shutdown; caller disappears and owned input destructor faults with a faulting panic payload | Job::Drop catches/takes captured operation before automatic permit release; next same-instance operation waits through actual input cleanup; payload destructor stays uncalled even without its waiter. The separate nonfaulting queued cleanup witness checks typed join interruption. | queued_shutdown_input_drop_fault_keeps_slot / central queued_abort_capture_cleanup_keeps_slot / queued_abort_fault_payload_is_not_dropped |
|24 | Actual storage effect returns a ready result; owned input destructor then faults, with its caller awaiting or detached | Captured-operation Drop is caught separately while permit is held; dispose ready result under a separate catch and report interruption, never acknowledgement/absence. | ready_effect_input_drop_fault_is_uncertain / detached_ready_input_drop_fault_is_contained / central ready_capture_fault_downgrades_and_disposes_output_under_slot |

|25 | Admission is acquired but never submitted, then dropped | Empty admission releases its instance slot; no operation or capture exists. | central unused_admission_releases_slot |
|26 | Synchronous submit returns its awaitable, which is dropped before first poll | Job was already handed to the captured executor; operation/capture cleanup remains under the slot through completion. | central unpolled_result_drop_retains_physical_slot |

Shared mechanism witnesses live beside `nessa-local-storage/src/physical_operation.rs`;
actual adapter I/O, owned clone/capture construction and typed outcomes remain in
the SDK/server feature tests. The test-only shared fixture observes real captures
and does not implement admission, executor selection or panic containment.

## Local evidence and measurement scope

The first red source was dirty over f1682e8a763646c6c3e73dee73c1c47987e2cbae:
test-only gates and five representative tests, with production still inline.
Independent OS watchdogs released real storage operations after sampling the
current-thread heartbeat immediately before release. All five reported 1 -> 1.
The [initial files and tracked patch](../../verification/bounded-persistence/evidence/first-red-provenance.tar.gz)
include hashes and precise Cargo/direct-binary provenance. The
[verification manifest](../../verification/bounded-persistence/README.md) distinguishes
historical source, current probes, raw measurements and final checked identity. No clean red commit
is claimed. The initial worker implementation passes those same five tests.
Extended matrices, mutation results and benchmark results are recorded separately.

The current graph has a global 128 retained-request cap. The manual SQLite
benchmark therefore measures legal single-store 32/256/2048 root-lifetime rows,
with zero spawns, and separately closed-root history and an empty snapshot.
Fixtures verify domain restoration. CSV records actual lifetime/spawn/settlement/
report/live counts, encoded body bytes, one adapter instance, concurrency and raw
end-to-end nanoseconds. It does not claim 2048 retained reservations. Measurements
are local filesystem/runtime results, not production latency or network timings.

## Physical panic boundary

The private Job owns a captured operation in Option and the admission permit.
It catches the operation call with the capture still owned, then separately
catches captured-operation destruction while holding admission. A ready result
whose captured cleanup faults is disposed under another catch and becomes typed
interruption. Job::Drop also catches remaining capture destruction when a queued
blocking job never starts. Each caught panic payload is forgotten without reading
or formatting it, so cleanup containment does not depend on the caller still
awaiting a JoinHandle. Join failures still map conservatively at the adapter.

These catches do not prevent a process abort caused by two faults in a physical
operation's own local guards during unwinding. They do not promise a lifetime for
returned-result destructors after Job finishes, whether delivered or discarded after caller loss, or suppress an external
panic hook. They introduce no public supervisor, retry, ledger or shutdown drain.

Manual benchmarks live in SDK tests/infrastructure/session_storage and server
tests/mcp_authorization/infrastructure as ignored integration tests. They link
the production library without private cfg(test) fault/probe instrumentation.
Measurements use 1000 samples per scenario (including four scheduled callers)
and disclose that limited tail sample size; no CI timing thresholds are added.
