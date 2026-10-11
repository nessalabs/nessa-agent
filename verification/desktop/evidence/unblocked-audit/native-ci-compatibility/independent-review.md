# Independent native test-fixture source review

Reviewer: `/root/unblocked_panel_list`, independent of coordinator/patch author.
Checkout: `/Users/nessa/.codex/worktrees/client-event-delivery/nessa-agent`
Exact base/head: `5c1c2257c630a3f5d5dc83ba422c882ecc598fbd` plus the uncommitted single-file patch.
Actual reviewed source SHA-256: `865a30b8890569d80d3127eadb918408e2bf99684bb87bec2f7cdfc8ade7b1da`.
Actual diff inventory checked: only `src-tauri/src/gateway/infrastructure/commands.rs` modified, within `#[cfg(test)] configuration_directory::UnconfirmedSettings::update`. No source edits or repo mutations by reviewer.

## Verdict

No findings at blocker, major, minor or nit priority in this scoped source review. This is independent source review, not a claim that compilation, runtime regressions or cross-platform CI have passed. Required scoped native tests, formatting and all-targets Clippy remain pending quiet release.

## Inspected scope and agreement

Read actual dirty diff, `UnconfirmedSettings` fields/update/load/load_service (lines680–722), all its constructors/callers (publication rollback matrix lines727ff, unsupported adapter lines778ff), native settings-change context, canonical review/agreement/organization gates, current architecture/structure/typed DI maps, author self-review and source-identity evidence.

The authoritative fixture counter is the existing AtomicUsize. Its related facts are: successful inner settings write, ownership of one configured acknowledgement fault, error or panic after that committed write, and subsequent rollback/retry success. No field or owner is added. Checked-decrement and counter ownership remain in this existing injected SettingsStore fixture, with actual native application/host assertions consuming it.

Mechanically compared prior fetch_update(SeqCst,SeqCst, checked_sub) with new load/checked_sub/compare_exchange loop:

- `inner.update(change)?` still runs once before fault accounting; an inner error returns before touching the counter.
- Existing skip_callback branch still returns the original inner.load before either update or counter access.
- A zero load makes checked_sub return None and breaks false; it neither underflows nor injects a fault. This matches prior fetch_update closure refusal, whose is_ok was false.
- A positive count injects a fault only after one successful CAS to count-1. The success linearization owns exactly one decrement before existing panic/error logic.
- On another caller changing the count, compare_exchange returns its current value; the loop reevaluates checked_sub from that value. Failed comparisons do not decrement and cannot turn an observed zero into a configured refusal. Strong CAS removes only the old primitive's permissible spurious weak retries, not any observable accounting outcome.
- Initial load and both CAS orderings are SeqCst, retaining the prior ordering strength. This is valid for AtomicUsize on32/64-bit targets, with checked subtraction preventing unsigned underflow.
- Existing panic_after_save assertion remains after successful fault consumption, so a caught acknowledgement panic consumes the same fault before retry. Existing io error and successful Ok(updated) origin are unchanged.

Read the existing public native matrix `an_unconfirmed_settings_publication_restores_its_known_prior_before_native_dispatch`: counts1/2 crossed with ordinary error/panic, persisted prior restoration, unchanged held live directory and zero native causes. Its successful retry/rollback after the first consumed fault reaches the zero-counter behavior; counts2 preserve uncertain rollback reporting. `settings_success_without_applying_the_owned_change_does_not_dispatch_native_work` covers the unchanged skip_callback branch. These are the appropriate pending actual regression checks; I did not execute them or claim their results.

## MSRV, production and organization

Read effective app metadata directly: `src-tauri/Cargo.toml` independently declares rust-version1.85 (not merely inherited workspace assumption); root workspace likewise declares1.85. The replacement uses existing stable load, checked_sub, compare_exchange, let-else and SeqCst APIs supported by that floor. No try_update/newer API, warning allowance, version/ABI/toolchain policy or dependency change is introduced.

Independently asserted the production prefix before configuration_directory's cfg(test) module is byte-identical to base. Git diff proves the sole changed block sits inside that module. No production authority, domain decision, native admission/persistence/rollback policy, command mapping, public surface or user-visible behavior changes. No path/layer/link moves; existing maps and existing native ownership/order table remain valid. This is a mechanical test-fixture expression of the existing rule, with no new state or ordering requiring a duplicated design table.

## Explicit limits

No compiler, test, browser, GUI or runtime probe started: the frozen browser delegate owns quiet whole/retained/focused verification. I did not reproduce the latest CI deprecation independently; actual denied-warning logs are author/coordinator evidence. This review does not reopen broader native/SDK ownership reviews or claim all3platform/-Dwarnings compatibility before their actual reruns. The coordinator must complete the pending scoped native public suite, format/Clippy and renewed CI on the patched source before closing the compatibility failure.
