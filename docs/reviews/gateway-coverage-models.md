# Gateway call and same-person model evidence

The source checkpoint is app main at `9a3253951826e2ae0e5666f34298a8c9093ca9d7`.
The relevant source files match that commit in the review branch. The checks
ran on 2026-11-25 with the pinned TLC 1.7.4 JAR through Nessa Docs
`108883365cc01e9c8996423739b65ef1c29acd8c`. The canonical service uses one TLC
worker and limits each run to 2,000 distinct states and 20 seconds.

| Baseline | Distinct states | Safety rules held | Spec/config digest |
| --- | --- | --- | --- |
| CLI and MCP call | 100 | 5 | `1d4754b8b7ec5e0db14cd21802931e281f95d6d484ef9bf3dbcf5c66b9dee080` |
| Same-person attribution | 20 | 6 | `050b404fe60ad038d93613382c3c625154168a6abcaf0550f34610a2d1910f07` |

The call model covers two serial invocations over up to two sockets. Its inputs
represent local catalog validity, Cedar admission, and a correlated or
uncorrelated reply. A single loss transition covers unusable responses and
timeouts. Gateway command and local transport source establish the catalog
and admission order, response correlation and retirement. A separate
reviewer checked the model against the adapter's retirement and reconnection
path. The call page now identifies the grouped loss behavior and the
session-only method outside the model. [Call model scope](../state/services/gateway/call-a-method-from-the-cli-or-mcp.md#model-coverage)

The attribution model covers one invitation. A separate source pass compared it
against the credential registry's publish order, the grant store's link and
audit transaction, the held-grant lookup and the expiry rollback fixture. The
model tracks recorded credential issuance independently from the published
link, and applies the removal receipt at the attempted write after rollback.
The model excludes replacement enrollment, combined expiry/revocation timing
and detailed initiator evidence. This review of attribution was performed by
the model author. [Attribution model scope](../state/services/gateway/same-person.md#model-coverage)

The targeted check ran both baselines and the twelve in-memory controls
defined in the gateway probe registry. Each control broke the listed rule.
No baseline reported a broken or unchecked rule, and no control reported an
unchecked rule. The repository-wide reproduction commands are
`pnpm docs:state:models` and `pnpm docs:state:probes` with the renderer and
checker configured. These runs exercise finite abstractions. The focused Rust
regressions were read during source review. [Check setup](../../scripts/state-docs/README.md)

| Mutation | Broken rule | Counterexample |
| --- | --- | --- |
| Permit catalog rejection to dispatch | `CatalogBeforeSend` | An invalid call reaches Sent. |
| Treat a Cedar denial as allow | `CedarBeforePayload` | An invocation denied by Cedar receives a payload. |
| Allow a mismatched response to settle | `ReplyMustMatch` | A reply with the wrong ID settles the sent call. |
| Allow dispatch on the retired socket | `LossRetiresSocket` | The second call sends on the first call's retired socket. |
| Collapse passphrase loss into a transport error | `LostVaultIsUnconfirmed` | A lost passphrase reply reports an ordinary transport error. |
| Accept true from an Integration principal | `HumanChoiceOnly` | The invitation stores true with Integration provenance. |
| Write the link without audit | `LinkNeedsActivationAndAudit` | Activation completes and attribution commits without the audit. |
| Write the link before activation | `LinkNeedsActivationAndAudit` | Attribution commits from the saved choice before credential issuance. |
| Replace removal cause with expiry | `RemovalKeepsCause` | The removal receipt reports expiry for a revoked credential. |
| Ignore the receipt after a clock rollback | `RetiredChoiceStaysConsumed` | A receipt records removal, then rollback permits the same invitation to link. |
| Substitute attribution for a grant | `AttributionNeverGrantsRead` | The link admits a read with no grant. |
| Ignore the grant binding | `AttributionMatchesBinding` | A read reports the person when the grant binding differs. |
