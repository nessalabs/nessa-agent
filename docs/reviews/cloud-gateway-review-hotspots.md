# Where recent reviews found state errors

Reviewed main at `33da134e2a334e05c385d54cca6f6284acc0448e`. The sample is the 20 most recently created PRs returned by GitHub, ending at #1439. Ten had inline feedback. Their 29 threads contain 53 comments including replies. Each count below assigns a thread to its original file. This measures review attention. It cannot estimate defects per change without change-volume and reviewer-coverage data.

| File | Threads | Comments including replies |
| --- | ---: | ---: |
| `crates/nessa-conversation/src/workspaces/application/cloud_boxes.rs` | 5 | 10 |
| `docs/guides/headless-gateway/nessa-gateway-bootstrap` | 5 | 10 |
| `src/desktop/workspace/adapters/gateway/gateway-registry-adapter.ts` | 3 | 6 |
| `crates/nessa-gateway/src/terminal/engine.rs` | 3 | 6 |
| `crates/nessa-device-pairing/src/infrastructure/connection.rs` | 1 | 1 |
| `crates/nessa-gateway/src/product/native.rs` | 1 | 1 |
| `docs/state/services/sync/failure.md` | 1 | 1 |
| `crates/nessa-server/src/composition/local_auth.rs` | 1 | 2 |
| `packages/nessa-client/src/presentation/cloud-api.ts` | 1 | 2 |
| `crates/nessa-conversation/src/workspaces/infrastructure/cloud_boxes.rs` | 1 | 2 |
| `src/desktop/workspace/adapters/gateway/multi-gateway-source.ts` | 1 | 2 |
| `src/desktop/dependencies.ts` | 1 | 1 |
| `src/desktop/workspace/adapters/gateway/multi-terminal-gateway.ts` | 1 | 1 |
| `crates/nessa-server/src/composition/root.rs` | 1 | 2 |
| `docs/guides/headless-gateway/README.md` | 1 | 2 |
| `crates/nessa-conversation/src/application/service.rs` | 1 | 2 |
| `crates/nessa-gateway/src/product/voice.rs` | 1 | 2 |

## What to change first

1. **Make idle evidence reliable, then close the admission race.** The bootstrap helper now requires complete, typed activity lists or positive evidence that the unit is loaded and inactive. It scans conversation rows directly. The [sleep model](../state/services/gateway/cloud-box-sleep.md) still finds an idle snapshot followed by new work and provider stop. Move the decision to the gateway that admits conversations and terminals. Test admission racing with drain acquisition, provider refusal, a lost reply and recovery after restart. Keep wake, retries, visibility of uncertain outcomes and audit records. This coordination remains proposed.
2. **Give cloud-box receipt validation one owner.** The file store validates decoded histories, and the service validates the same histories again in list, create, wake, sleep, rename and delete. A validated history type constructed on reads could carry the invariant and expose the latest state. Keep filename/box-ID matching at the file boundary. Measure list latency and allocations across increasing box counts and history lengths before refactoring. Preserve incomplete provisioning recovery, repeated delete, rename audit evidence and old on-disk receipts as acceptance cases.
3. **Test remote reconnect with two distinguishable gateways.** PR #1424 has two unresolved threads, on attachment routing and rebinding a terminal opened offline. First check current behavior with identical local IDs on two gateways, then disconnect and reconnect one. Assert the bytes and terminal events come from their original gateway. This turns a recurring ownership question into a test that survives adapter changes. The review status alone does not establish a current defect.
4. **Keep permit retirement in the owner that releases capacity.** PR #1436 already fixed the pool teardown ordering. The new [pool model](../state/services/gateway/pool-membership.md) checks delayed listener completion and replacement admission. Its regression controls restore stale registry membership and omit registration. This keeps the reason for the existing destructor visible to the next agent editing it.

## Verification and limits

TLC 1.7.4 explored 20 distinct sleep states and 26 distinct pool states. All five configured safety properties held, and each of the five isolated mutations broke its intended property. A separate sleep run checked the stronger guarantee and produced the admission counterexample. An independent source review caught a probe that failed to mutate and a missing weak-reference pruning step. Both were corrected before the final runs. The linked pages contain the checker receipts and model bounds. The bootstrap regressions invoke the real script with stubbed command responses. No live provider was exercised and no end-to-end performance improvement was measured.

## Inline review evidence

- [#1436 · Retire the wake endpoint before returning Product capacity](https://github.com/nessalabs/nessa-agent/pull/1436#discussion_r4684585481)
- [#1434 · Import the probe types at module scope](https://github.com/nessalabs/nessa-agent/pull/1434#discussion_r4680180697)
- [#1431 · Retain recoverable ownership when provisioning cleanup is uncertain](https://github.com/nessalabs/nessa-agent/pull/1431#discussion_r4678024089)
- [#1431 · Allow deletion retries after a terminal receipt was saved](https://github.com/nessalabs/nessa-agent/pull/1431#discussion_r4678024104)
- [#1431 · Preserve the old and new names in rename audit evidence](https://github.com/nessalabs/nessa-agent/pull/1431#discussion_r4678024115)
- [#1431 · Issue a credential with the permissions Desktop requires](https://github.com/nessalabs/nessa-agent/pull/1431#discussion_r4678212173)
- [#1431 · Check activity across the gateway, not just the bootstrap owner](https://github.com/nessalabs/nessa-agent/pull/1431#discussion_r4678212184)
- [#1429 · Distinguish identity changes from reset-required failures](https://github.com/nessalabs/nessa-agent/pull/1429#discussion_r4677328123)
- [#1425 · Provision an image that includes the gateway bootstrap contract](https://github.com/nessalabs/nessa-agent/pull/1425#discussion_r4671944514)
- [#1425 · Refuse sleep when activity evidence is unavailable](https://github.com/nessalabs/nessa-agent/pull/1425#discussion_r4671944529)
- [#1425 · Give provisioning a deadline that covers the provider operation](https://github.com/nessalabs/nessa-agent/pull/1425#discussion_r4671944542)
- [#1425 · Avoid exhausting a persistent box's receipt history](https://github.com/nessalabs/nessa-agent/pull/1425#discussion_r4671944558)
- [#1425 · Recover or roll back boxes when Desktop enrollment fails](https://github.com/nessalabs/nessa-agent/pull/1425#discussion_r4674213781)
- [#1425 · Surface audit failures on refusal and cleanup paths](https://github.com/nessalabs/nessa-agent/pull/1425#discussion_r4674213803)
- [#1425 · Produce endpoint metadata before requiring it during bootstrap](https://github.com/nessalabs/nessa-agent/pull/1425#discussion_r4674432387)
- [#1425 · Recover the provisioned VM when the admission audit fails](https://github.com/nessalabs/nessa-agent/pull/1425#discussion_r4674432401)
- [#1425 · Connect the busy probe to live gateway activity](https://github.com/nessalabs/nessa-agent/pull/1425#discussion_r4674432407)
- [#1424 · Preserve remote sessions when a gateway index is unavailable](https://github.com/nessalabs/nessa-agent/pull/1424#discussion_r4671062750)
- [#1424 · Verify the gateway identity on every new connection](https://github.com/nessalabs/nessa-agent/pull/1424#discussion_r4671062760)
- [#1424 · Supply attachment byte access to remote workspace sources](https://github.com/nessalabs/nessa-agent/pull/1424#discussion_r4671062769)
- [#1424 · Route remote attachment reads through their owning gateway](https://github.com/nessalabs/nessa-agent/pull/1424#discussion_r4673661011) (unresolved at review)
- [#1424 · Rebind terminal views created while a gateway is offline](https://github.com/nessalabs/nessa-agent/pull/1424#discussion_r4673661018) (unresolved at review)
- [#1423 · Restore saved GitHub connections after headless unlock](https://github.com/nessalabs/nessa-agent/pull/1423#discussion_r4670096119)
- [#1423 · Supply the provisioned owner credential in pairing commands](https://github.com/nessalabs/nessa-agent/pull/1423#discussion_r4670096133)
- [#1416 · Recheck setup while waiting for the selection lock](https://github.com/nessalabs/nessa-agent/pull/1416#discussion_r4661187444)
- [#1415 · Clean up the remote process before releasing its terminal guard](https://github.com/nessalabs/nessa-agent/pull/1415#discussion_r4657497457)
- [#1415 · Preserve the output polling deadline across incoming commands](https://github.com/nessalabs/nessa-agent/pull/1415#discussion_r4657497462)
- [#1415 · Drain every remaining output page before publishing exit](https://github.com/nessalabs/nessa-agent/pull/1415#discussion_r4657497467)
- [#1414 · Route new-chat voice admission through Cedar](https://github.com/nessalabs/nessa-agent/pull/1414#discussion_r4657307652)

Sampled PR numbers in descending creation order: 1439, 1438, 1437, 1436, 1435, 1434, 1433, 1432, 1431, 1430, 1429, 1426, 1425, 1424, 1423, 1422, 1416, 1415, 1414, 1413.
