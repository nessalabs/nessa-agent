# Attachment gesture destinations

Issue #824 is separate from placement/resource ownership in #816. The panel's
local transient state uses a frozen destination containing the actual Redux
store, conversation ID, original resource manager, and existing attach command.
Capture it at the gesture boundary, before a picker, host event, folder walk,
image read, or URL fetch can wait. It is local UI correlation, not gateway
authorization. Delayed decisions read the captured store's live conversations
and keep the original resource effects while that conversation remains open.
Only the matching current store and conversation may render transient state;
valid delayed results still publish to their captured live destination.

Each live picker, image read, or URL fetch is one complete operation value
shared by a ref (for synchronous admission) and state (for rendering). Its
object identity owns cleanup, so an older completion cannot clear a later
operation. Folder walks follow the same per-operation ownership rule and abort
every active controller on unmount. Host-minted drop metadata uses a bounded
64-entry active-destination map: each accepted batch keeps its captured owner
until the terminal drop result consumes the entry. A 65th active batch is
refused on its captured destination, and a terminal result frees its slot.
Active picker destinations live with their picker operation until that command
settles. Unknown or already-consumed drop names are ignored rather than
assigned to the active tab. No epoch, cancellation flag or global registry is
used.

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
| AG2 | A picker held; B starts picker; results and readiness events arrive in reverse order, with more than 64 picker requests still active | Each batch tile and returned file stay with its captured store; active picker metadata is never evicted; A cleanup cannot clear B | owner hook and host command |
| AG3 | A image read held; B starts read; B settles first; A is still held | A remains pending after B finishes, then attaches to its original live destination | owner hook |
| AG4 | A URL fetch held; B fetch starts; A succeeds/fails first | B pending/notice/controller remain intact | owner hook |
| AG5 | Native batch begins in A, provider changes, host readying/drop arrives | Whole destination survives join; valid live result applies to A, with no B tile, notice or focus | owner and host-drop hooks |
| AG6 | Old subscribed callback fires after owner replacement; several owned reads are active when the hook unmounts | Disposed callback publishes nothing; every active controller is aborted and all operations retire | owner hook |
| AG7 | Same store changes tabs, then work completes; original tab closes | Attach original open tab; existing closed-tab notice remains on that store | existing picker/readying suites |
| AG8 | Component unmounts while URL/read waits | Existing cancellation and no retired UI publication | owner hook |
| AG9 | Folder read held in A; B/c0 starts a folder read; B settles first | A remains pending and attaches to its original live destination; unmount aborts every walk | folder owner regression |
| AG10 | Two same-owner reads overlap | Existing reading refusal; first admitted operation still owns cleanup | existing races and owner hook |
| AG11 | Drop begins in A, provider switches to B, result arrives while B is current | Valid result attaches to A's original live destination; B currentness gates only reveal and presentation | App and host-drop regression |
| AG12 | Picker remains open; attachment hook unmounts; picker returns an image | Teardown retires the operation owner and closes admission; no native read or resource effect starts afterward | owner hook |
| AG13 | Capture A/c1; replace its provider with B/c1; close A before picker preflight or a held image read settles | A's closed-destination notice is not redirected to B; same-owner closure still uses that owner's active tab as in AG7 | picker preflight and held-read regressions |
| AG14 | Browser picker verification changes providers before the second gesture | Wait for the committed replacement provider; its empty draft must not start an unrelated upload or rename the attachment control | picker-owner browser scenario |
| AG15 | 64 drop batches are active; a 65th begins, then an older result arrives | Keep all 64 destinations until their own terminal results; explicitly refuse the 65th on its captured destination, then admit a new batch after one result is consumed. Duplicate and unknown outcomes do nothing; a late result still attaches to its original live owner | actual App 65-drop saturation regression |

The host-drop adapter carries the captured destination as one value; it does
not resolve it to a local string and hand that to a replacement hook. The App
reveals a draft only for a destination owned by its current store.
