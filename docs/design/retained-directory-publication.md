# Retained-directory publication (#324)

Issue: [324](https://github.com/nessalabs/nessa-agent/issues/324).
Status: accepted design; implementation and platform evidence in progress.

## Owners

`nessa-local-storage::PrivateDirectoryTempFile` owns reservation identity,
pre-rename checks, native publication, immediate cleanup disarm and typed
acknowledgement. `PublishedPrivateFile` retains the exact destination, native
identity and open payload handle after rename. Windows retains its private
non-delete-sharing directory chain and renames through the original DELETE
reservation handle using one `FILE_RENAME_INFO` encoder. `publish_new` selects
exclusive publication; `replace` selects `FileRenameInfoEx` with POSIX semantics
through the same encoder as path-based replacement (#874). Existing delete-sharing
readers keep the old file while new opens observe the replacement. Mutation-capable
handles that omit delete sharing still pin their names; no ignore-read-only flag
or path fallback is used. Tests are in the storage owner and retained-directory
integration suite, indexed by the crate README.

The reservation carries two handles: original rename/cleanup authority and writable
payload. Public mutable file access can replace the payload slot. The shared
publication owner validates their native identities before flushing that payload
or renaming, using the existing platform identity owner. Original name validation
precedes this comparison; the captured original identity is reused afterward.
A same-object writable clone is valid. No content-immutability or application CAS
guarantee is implied by matching file identity.

The consumer owns application authorization and expected-old CAS under its
stable lock. Storage replacement can also publish to an absent name; it does not
establish that an application expected an existing record. Pairing consumption,
writable exact retry/reconciliation and its original pending operation remain
owned by #264, outside this shared-storage slice.

## Ordering table (written before source changes)

| Row | Event/order | Result and owner | Evidence |
| --- | --- | --- | --- |
| W01 | Current private reservation and closed existing destination | Replace with reservation identity; acknowledge flush and bindings; remove reservation name | `retained_replacement_publishes_the_reserved_identity_atomically` |
| W02 | Destination absent | Replacement publishes name; exclusive publication still refuses occupied name | `retained_replacement_accepts_an_absent_destination`; existing exclusive-publication test |
| W03 | Invalid destination, changed origin or reservation before rename | Existing typed checkpoint, no published fact; clean only matching origin reservation, report independent cleanup error | `replacement_invalid_name_keeps_original_destination`; existing origin/reservation tests; `replacement_changed_reservation_retains_foreign_name` |
| W04 | Application destination changed before expected-old read/CAS | Consumer refuses before reserve/rename; storage supplies no second CAS | #264 pending-store tests; excluded from this storage slice |
| W05 | Windows destination has mutation-capable open handle, or target is directory | Native Rename refusal, no published fact; origin reservation cleaned; valid replacement after obstruction released | `windows_replacement_refuses_a_pinned_destination_then_accepts_after_release`; `replacement_refuses_a_directory_destination` |
| W06 | Rename succeeds, caller loses answer or acknowledgement fails | Disarm reservation cleanup before any later fallible call; keep exact published file; Drop cannot remove destination | `native_replacement_failed_acknowledgement_retains_published_identity`; `native_publication_panic_after_rename_keeps_destination` |
| W07 | Each post-rename acknowledgement checkpoint fails | Preserve stage, source, published identity/name/handle; no cleanup error | `every_post_rename_failure_retains_name_identity_and_open_handle_without_cleanup`; actual native replacement test |
| W08 | Live consumer reconciliation | Match original published identity and exact bytes under same lock before acknowledgement | #264 consumption; excluded from this slice |
| W09 | Caller lost during asynchronous save | Consumer physical closure keeps storage/key/permit until drain; storage itself is synchronous | #264 owned worker tests; excluded from this slice |
| W10 | Reopen after physical process drain | Consumer reacquires private authority and reconciles exact durable operation; no invented old native identity | #264 restart tests; storage `retained_replacement_publishes_the_reserved_identity_atomically` reopens and reads |
| W11 | Unpublished temporary is dropped or explicit failure occurs | Origin-only best effort Drop; explicit failure reports cleanup failure independently; foreign same-name file preserved | existing reservation cleanup tests; `replacement_changed_reservation_retains_foreign_name` |
| W12 | Same-user leaf mutation inside final check/effect interval | Cooperating callers excluded by consumer lock; no native compare-and-swap claim | documented exclusion; Windows directory/lock lifetime tests |
| W14 | Valid Unicode extended destination path beyond MAX_PATH | Same validated encoder accepts exclusive publication followed by replacement; original private checks apply | `windows_publication_accepts_a_unicode_destination_beyond_max_path` |
| W13 | Process crash or power loss | Reopen may observe old/new/missing/corrupt; no automatic repair or fabricated result | normal reopen tests; power-loss experiment excluded |
| W15 | publish_new receives a valid private foreign payload through public as_file_mut replacement | ValidateReservation before payload flush/rename; published absent. Destination stays absent, foreign file stays intact; clean only original reservation | `publish_new_refuses_foreign_writable_payload_before_effect` |
| W16 | replace receives that foreign payload while original destination exists | Same pre-effect refusal; original destination identity/bytes and foreign file remain unchanged; original reservation cleanup only | `replacement_refuses_foreign_writable_payload_before_effect` |
| W17 | Ordinary writes or public slot replacement with try_clone of SAME original payload | Both operations remain accepted; payload/native authority/published destination all identify the same object | `same_original_writable_clone_remains_accepted_for_both_operations`; ordinary replacement/publication tests |
| W18 | Original reservation name changed before publication, with foreign payload also substituted | Original name check keeps precedence: ValidateReservation; independently retained cleanup failure; foreign same-name witness/destination remain untouched | `changed_origin_keeps_precedence_with_substituted_payload` (Unix namespace mutation); Windows retains original name-pinning tests |
| W19 | Destination has an ordinary delete-sharing reader during replacement (#874) | Publish reserved identity while that reader retains old bytes; new opens see complete replacement | `retained_replacement_keeps_a_reader_on_the_previous_record`; path-based `replacement_keeps_a_delete_sharing_reader_on_the_previous_record`; endpoint `publication_replaces_an_endpoint_while_a_discovery_reader_retains_the_old_record` |


Windows directory and mutation-capable handles pin names while held. A locked
replacement target is refused; releasing that obstruction admits ordinary
replacement. Existing verified read-only probes share deletion. Creation ACLs
remain protected current-user plus LocalSystem before bytes, with owner, regular
single-link and no-reparse checks on the same handles used for I/O.

## Windows protected restore directory candidate (#270)

The Windows-only native directory module in `nessa-local-storage` owns the
candidate mechanics; backup owns whether they are qualified for activation.
The existing backup fail-closed path stays in place until native tests prove
this candidate. Native CI [39701307415](https://github.com/nessalabs/nessa-agent/actions/runs/39701307415)
at `5f8d` disproved the original N03 assumption: the initial handle could not
create a child under a read/search-only directory DACL (access denied, OS 5).
That failed receipt remains failed. The revised candidate permits additions,
but never DELETE_CHILD, during copying. It seals all directories before final
whole-tree content and identity validation. The regression requires the original
N03 operation to remain denied after sealing; mutation through a pre-acquired
ADD handle must also fail. A granted handle alone is not proof of mutation rights.

Native `c76d` [39703824781](https://github.com/nessalabs/nessa-agent/actions/runs/39703824781)
also failed qualification: actual leaf deletion and post-seal subdirectory
creation succeeded, and the Win32 relative rename returned OS 87. These failed
receipts remain failed. The relative rename candidate now uses
`NtSetInformationFile(FileRenameInformation)` with a retained target handle and
a buffer at least the native structure size. Bounded read-only ACL/token
diagnostics accompany the unchanged mutation assertions; no SYSTEM grant or
test permission is changed on a guess. Backup activation stays unavailable.

| Order | Operation | Required result / failure |
| --- | --- | --- |
| N01 | Create one absent child with `NtCreateFile(FILE_CREATE)` and protected read/search plus ADD_FILE/ADD_SUBDIRECTORY user DACL, without DELETE_CHILD | Return initial ADD/DELETE/WRITE_DAC authority; collision preserves the existing object |
| N02 | Another process attempts actual root rename/removal, nested-directory removal or leaf mutation | Root initially shares only READ; nested directories share DELETE but neither their own ACL nor their parent grants DELETE. Leaves have read-only user ACLs from creation; no DELETE_CHILD exists in the copied tree |
| N03 | Create children relative to retained append-only directory; seal every directory read/search-only before final enumeration and content/identity verification | Child creation works before seal; the original N03 create is denied after seal, including through pre-acquired ADD_FILE/ADD_SUBDIRECTORY handles. Pre-acquired ADD authority cannot rename an external source into the sealed tree. Any unexpected entry fails final validation |
| N04 | Rename retained child handles before sealing, without replacement; after seal and final validation, close descendant handles and rename root through its initial DELETE handle | Preserve occupied destinations. Never reopen source for DELETE. An attempted directory rename with open descendants must return AccessDenied, per FILE_RENAME_INFORMATION. Post-close mutation refusals must hold before publication; no root ACL DELETE grant is added speculatively |
| N05 | Delete through the exact retained object handle | Remove that object only; a replacement at its old name is not cleanup authority |
| N06 | Successful root promotion, then reopen descendants read/control-only relative to the retained root and release temporary restrictions; retain root WRITE_DAC authority | Restore ordinary private owner ACLs only after live rename; failure remains an explicit error |
| N07 | Any create/rename/delete fails | Return the OS error; no path fallback, automatic ACL relaxation, or success claim |
| N08 | Fetch acquires an existing private sessions directory before creating any file | Acquire a no-delete-sharing handle; a pre-existing DELETE opener refuses acquisition. Snapshot its exact protected DACL through that handle before changing it |
| N09 | Existing directory has a pre-acquired DELETE_CHILD handle; fetch changes its current DACL to append-only before creating a read-only-ACL leaf | Actual deletion/relocation of the new leaf through that pre-acquired handle must fail. A granted handle without a successful mutation is not evidence of a bypass |
| N10 | Fetch seals current directory, validates output/sidecar identities and content, completes availability, then releases protection | Restore the exact captured DACL on its retained directory object. Verify ACL bytes; no ordinary private-default ACL substitutes for the original. On failure report retained protection |

| T01 | Capture the current thread token, or its absence, before impersonation | Preserve the exact previous TokenId and restore it; an absent token is restored to absence |
| T02 | Derive DISABLE_MAX_PRIVILEGE token, without disabling SIDs or adding restricted SIDs, and install on this thread only | Same user and group SIDs/attributes; effective backup/restore disabled; no process token mutation |
| T03 | Full-ACL ordinary writer attempts create/write/rename/delete before protected assertions | All actual operations succeed; protected negative assertions run unchanged |
| T04 | Spawn a competing writer | Install its own scoped ordinary token; impersonation is not inherited |
| T05 | Test exits or unwinds | RAII restores exact prior thread token; restoration errors fail visibly |
| P01 | Repeat post-seal subdirectory creation under original effective token, then ordinary token | Enabled backup/restore profile reproduces the privileged bypass; otherwise original refusal is asserted. Restricted refusal is asserted under both profiles; no skipped branch |

Native ce5d CI 39706336820 confirmed current user is not SYSTEM, the SYSTEM ACE
is nonmatching, and backup/restore privileges are enabled on the elevated runner.
Its correct DACLs therefore do not establish the ordinary-writer boundary.
The scoped tests preserve this privileged counterexample explicitly. They do
not claim protection from an administrator using backup/restore privileges.
Directory rename with open descendants is independently unsupported by the
[kernel rename contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information).
[CreateRestrictedToken](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken)
provides the privilege-only test boundary; source user and groups remain checked.

The threat boundary excludes an administrator, process-handle duplication and
an owner deliberately changing the DACL, as Unix directory mode protection also
excludes an owner deliberately changing its mode. Native Windows tests must
exercise the candidate on Windows; Linux compilation does not qualify it.
The existing platform CI owns those tests. No power-loss durability is claimed.

## Durability and evidence limits

Retained publication flushes the writable payload before and after handle rename.
Windows `PrivateDirectory::sync` revalidates identity; it has no directory-fsync
guarantee. No retained write-through move or arbitrary power-loss survival is
claimed. Path-based publication does not acquire this owner's retained directory
authority; the shared rename encoder preserves each entry point's own checks.

Local macOS runtime tests and Windows target compilation are distinct evidence.
Actual Windows runtime tests use the existing local-auth platform CI selection
(storage/auth/server/SDK tests and all-targets Clippy); no new job is required.
Until that runs, Windows runtime behavior remains unverified. Hosted elevated
runner evidence does not establish ordinary-user permissions independently.

Primary references: [FILE_RENAME_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info),
[SetFileInformationByHandle](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle),
[CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
[FlushFileBuffers](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers).
