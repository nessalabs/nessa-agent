# 689 — Linux service-definition replacement

Issues #689 and #690 own this correction to the systemd adapter. The adapter
validates a retained definition by recovering its Claude directory from its
canonical bytes and rendering all its other ownership inputs from the desktop.
This accepts a prior directory without accepting arbitrary service bytes.
Generation reuse requires equality with the complete desired command, so a
changed PATH, Claude directory, runtime or other command input selects a new
incarnation before effects are admitted.

| Retained state / event                                                               | Decision and effect order                                                                                                            | Regression                                                       |
| ------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------- |
| Exact running unit, unchanged command                                                | Reuse generation; readiness corroboration; no stop/reload/start                                                                      | unchanged definition reuse                                       |
| Exact running unit, new PATH or Claude directory                                     | Fresh generation; admit before/after targets; retire exact prior; stop; publish; reload; start; confirm ready                        | command replacement selection and retained-definition validation |
| Exact inactive/unloaded definition, changed directory                                | Validate actual old directory; publish desired definition; reload; start                                                             | inactive prior directory validation                              |
| Quoted environment value contains a Claude-looking key / repeated Claude assignments | Parse complete arguments; select one unique assignment or refuse duplicates                                                          | unique environment-argument recovery                             |
| Foreign bytes or directory                                                           | Refuse before replacement; retain bytes/process                                                                                      | canonical owned-render rejection                                 |
| Settings publication pending / another reconciliation reads live configuration       | Hold the directory-publication claim without changing live configuration; publish live only after durable acknowledgement            | unconfirmed settings publication rollback                        |
| Settings save returns failure after its update callback obtained the prior value     | Retain prior publication authority, restore the desired value if it landed, refuse native dispatch and preserve any rollback failure | unconfirmed settings publication rollback                        |
| Native failure before replacing old definition                                       | Retain old definition; restore known durable and live prior values before releasing publication                                      | failed directory change rollback                                 |
| Failure after publication or desktop restart                                         | Recovery inspects canonical on-disk definition and journal before new effects; no replay of an uncertain command                     | existing recovered definition/effect tests                       |

Durable settings publication shares the existing `ClaudePublication` owner with
live configuration and reconciliation. The command supplies owned clones of the
existing gateway and settings store. On first poll, a request claims its slot and
starts one owned transaction; its caller is only a waiter. That transaction
retains the slot and prior values through the native receipt and final rollback.
Dropping a caller cannot roll back configuration while a detached native effect
still uses it. Identical requests join and different requests wait; even an
unchanged live directory holds the slot while saving. The settings adapter retains
the prior value inside the store callback, including an unconfirmed return or
panic after the callback. A rollback failure retains the original failure.
Restart recovery validates the actual installed canonical configuration, since
an interrupted native effect may have landed before failure was recorded.

| Publication state / event                                               | Owner and order                                                                                                                      | Regression                                             |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------ |
| Request never polled / caller drops                                     | No claim, save or native dispatch                                                                                                    | unpolled request has no effects                        |
| Accepted request / caller drops during durable save or native execution | Transaction keeps slot and desired values; caller removal has no effect on settlement                                                | dropped directory caller retains ownership             |
| Native success / caller absent or same-directory waiter                 | Keep durable/live desired; acknowledge receipt; release slot; equal retry performs no native duplicate                               | late success agrees with installed command and journal |
| Native refusal before effects / caller absent                           | Await actual receipt; restore known durable/live prior; retain rollback errors; release slot; next requested directory may reconcile | late failure and subsequent request agree              |
| Native success / outcome audit fails or panics                          | Keep acknowledged desired values; return typed audit failure containing physical success; do not claim native rollback               | late physical success with receipt failure             |
| Durable save, host publication or reconciliation panics                 | Catch outside the transaction; retain known prior values and finish joined receipts; catch each rollback independently               | publication phase panic corpus                         |
| Rollback adapter panics / another rollback remains                      | Retain panic beside primary failure; run the other rollback; finish publication before next request                                  | independent rollback panic corpus                      |
| Caller waiter panics or disappears / another waiter remains             | Waiter owns no publication effects; transaction continues and remaining waiter receives settled receipt                              | panicking waiter retains transaction                   |
| Held test effect / first receipt already waiting | Observe both receipt waiters before releasing the native/settings fixture; the first alone does not prove second admission | identical/different callers; unchanged save |

An equal save acknowledges no configuration change. If the gateway's startup
projection is Failed, it returns that existing typed failure and leaves native
recovery to explicit retry; it does not turn an uncertain/failed startup into a
successful registration receipt. General partial native failures remain owned by
the existing native journal/recovery protocol. Restoring local known priors does
not establish that an unknown daemon effect was undone.

| Publication state / event                            | Owner and order                                                                                              | Regression                                  |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ | ------------------------------------------- |
| Equal directory after failed native or audit receipt | Publish under the same owner; retain original startup failure; no native duplicate or recovery-success claim | equal save preserves failed startup receipt |

| Publication state / event                                            | Owner and order                                                                                                                                                              | Regression                                           |
| -------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| Native effect returns unconfirmed failure after changing the command | Preserve failed startup/result; restore known local priors without asserting daemon rollback; equal save returns existing failure; explicit retry owns native reconciliation | unconfirmed effect stays failed until explicit retry |
