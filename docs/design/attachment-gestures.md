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

The host announces drop beginnings only when the operating-system event names
paths. File and refusal results therefore require a reserved batch destination
and consume it once. A pathless text drop goes to the focused composer, which
is the current target for pasted prose. A pathless image URL captures the
current destination when its terminal event is delivered, before its fetch can
wait; it does not pretend that the host announced a reservation it never sent.

When the host picker is unavailable, one browser file input stands in for it.
The originating picker operation remains live until that input emits `change`
or `cancel`. Its result keeps the operation's captured destination across a
provider switch. Since the input has one chooser at a time, a second chooser
attempt while it is outstanding receives the existing picker-unavailable
refusal and cannot replace the first owner. Cancel releases the slot for a
later attempt; unmount retires it with the rest of the operation set.

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
| AG15 | 64 drops begin in A/c0; switch to empty B/c0; the 65th is rejected; an older result, duplicate, unknown and post-capacity result arrive | Keep all 64 destinations until terminal results; show the 65th refusal in B; ignore its rejected result and unknown/duplicate results; route the oldest result only to A and consume it once; admit the next batch into B after a slot frees | actual App 65-drop saturation browser scenario |
| AG16 | A host drop carries text or an image URL without paths and therefore without a batch-begun event | Text is pasted to the focused composer; image fetch is explicitly owned by the current destination at result delivery; path/refusal results still require one reserved batch | actual App host-drop regression |
| AG17 | Browser picker fallback opens for A; the provider changes to B before `change`; the chooser is attempted again, cancelled, reopened or torn down | A's delayed change attaches to A; a second attempt cannot replace A's owner; cancellation frees the single input; unmount retires its operation and ignores late events | actual App and hook browser-picker regressions |
| AG18 | A and B are open in one store; A's delayed refusal arrives while B is active, then A is revisited and answered | Retain one refusal per open local conversation in the current store/resource scope; A and B notices remain independent, and answering A leaves B's notice intact | owner hook regression |
| AG19 | A host listener registration is pending while the provider changes; the old callback fires after unmount | Keep one host subscription for the mounted component, read the latest destination from committed refs, and ignore callbacks after disposal without a registration gap | host-drop and owner hook regressions |
| AG20 | A folder walk is pending in provider A/c0; replace it with provider B/c0 and immediately send or drop a second folder without an intervening UI event | Query pending work against B's committed scope; capture B at the second folder's drop boundary; keep A's existing walk and result owned by A | actual App folder/send and folder/drop regressions |

Before a native folder result reaches that destination, the host walk reserves
queued siblings against `MOST_FOLDER_ENTRIES`. Its filesystem adapter bounds
iteration before collecting or sorting. The host tests retain the existing
whole-drop refusal and exact-limit behavior:

| Native enumeration ordering | Result | Regression |
| --- | --- | --- |
| A directory has more entries than the remaining walk budget | Consume at most the budget plus one overflow witness; refuse the folder rather than collect its full listing | `wide_directory_consumption_stops_at_budget_plus_overflow`, `disk_directory_iterator_observes_only_budget_plus_one_entries` in [`files.rs`](../../src-tauri/src/attachments/files.rs) |
| Earlier directory expansion has queued siblings before the next directory is read | Reserve those siblings before supplying the next enumeration budget | `queued_sibling_directories_reserve_the_remaining_budget` in [`dropping.rs`](../../src-tauri/src/attachments/dropping.rs) |
| Entries fit the exact remaining bound, or iteration reports an error | Sort admitted entries; preserve read errors and admit an exact-bound tree | `bounded_directory_sorts_exact_limit_and_preserves_read_errors` in `files.rs`; `exact_entry_limit_with_empty_directories_and_one_file_is_admitted` in `dropping.rs` |

This bounds consumed entries and retained listing size across the supported
native platforms. It does not bound the duration of one filesystem iterator
step or cancel a kernel-blocked operation; the existing filesystem-answer
deadline remains separate from enumeration accounting.

The host-drop adapter carries the captured destination as one value; it does
not resolve it to a local string and hand that to a replacement hook. The App
reveals a draft only for a destination owned by its current store.

Refusal state is local to the attachment hook and its store/resource scope, but
one global slot loses a still-open inactive conversation's message when another
tab refuses. Refusals therefore belong in a read-only map keyed by local
conversation ID, pruned when the conversation closes or its provider scope is
retired; only the active conversation's entry is rendered or dismissed. The
picker ownership suite covers independent notices, delayed inactive-tab
refusals, answer isolation, same-ID provider replacement, and close cleanup.
Likewise, host event subscriptions belong to the mounted UI boundary rather
than to a provider render: re-registering during a provider swap can leave an
async unsubscribe/register gap, while callbacks can already consult the latest
committed destination through a ref.
