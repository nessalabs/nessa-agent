# Attachment gesture destinations

Issue #824 is separate from placement/resource ownership in #816. The panel's
local transient state uses a frozen destination containing the actual Redux
store, conversation ID, original resource manager, and existing attach command.
Capture it at the gesture boundary, before a picker, host event, folder walk,
image read, or URL fetch can wait. It is local UI correlation, not gateway
authorization. Delayed decisions read the captured store's live conversations
and keep the original resource effects. Only the matching current store and
conversation may render transient state or receive publication.

The pending operation is one value shared by a ref (for synchronous admission)
and state (for rendering). Its object identity owns cleanup, so an older
completion cannot clear a later operation. Host batch entries retain the whole
destination for at most 64 names; an unknown or evicted name has no destination
and is ignored rather than assigned to the active tab. No epoch, cancellation
flag or global registry is used.

Picker readiness needs a host identity before its first event: picker calls may
overlap, and the separate batch event has no ordering contract with the command
result. The panel creates one validated picker request ID at the gesture
boundary, registers that ID with the captured destination before invoking the
host, and passes it as the command's required argument. The host's `Batch`
wraps that identity for the picked gesture and every readiness event, while
drop batches continue to be minted by the host. A picker entry is retired when
its command settles; any late readiness event for it has no destination and is
ignored. Returned files still use the captured destination from that invocation.
Picker request IDs are private to the panel hook. A newly generated ID is
registered before invoking the host picker, and an ID collision with an active
request is refused without replacing that request's captured destination.

| Row | Ordering | Expected behavior | Public regression |
| --- | --- | --- | --- |
| AG1 | A/c0 refuses; same mounted component changes to B/c0 | B sees no A refusal; B notice survives later A callback | owner hook |
| AG2 | A picker held; B starts picker; results and readiness events arrive in reverse order | Each batch tile and returned file stay with its captured store; A cleanup cannot clear B | owner hook and host command |
| AG3 | A image read held; B starts read; A settles first | B pending remains; original store receives A attachment, never B | owner hook |
| AG4 | A URL fetch held; B fetch starts; A succeeds/fails first | B pending/notice/controller remain intact | owner hook |
| AG5 | Native batch begins in A, provider changes, host readying/drop arrives | Whole destination survives join; no B tile, notice or focus | owner and host-drop hooks |
| AG6 | Old subscribed callback fires after owner replacement | Disposed callback publishes nothing | owner hook |
| AG7 | Same store changes tabs, then work completes; original tab closes | Attach original open tab; existing closed-tab notice remains on that store | existing picker/readying suites |
| AG8 | Component unmounts while URL/read waits | Existing cancellation and no retired UI publication | owner hook |
| AG9 | Folder read held in A; B/c0 starts a folder read; A settles | B spinner and operation survive A cleanup | folder owner regression |
| AG10 | Two same-owner reads overlap | Existing reading refusal; first admitted operation still owns cleanup | existing races and owner hook |

The host-drop adapter carries the captured destination as one value; it does
not resolve it to a local string and hand that to a replacement hook. The App
reveals a draft only for a destination owned by its current store.
