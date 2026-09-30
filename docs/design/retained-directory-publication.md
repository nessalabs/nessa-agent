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
exclusive publication; `replace` selects replacement. No path fallback or POSIX
sharing override is used. Tests are in the storage owner and retained-directory
integration suite, indexed by the crate README.

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

Windows directory and mutation-capable handles pin names while held. A locked
replacement target is refused; releasing that obstruction admits ordinary
replacement. Existing verified read-only probes share deletion. Creation ACLs
remain protected current-user plus LocalSystem before bytes, with owner, regular
single-link and no-reparse checks on the same handles used for I/O.

## Durability and evidence limits

Retained publication flushes the writable payload before and after handle rename.
Windows `PrivateDirectory::sync` revalidates identity; it has no directory-fsync
guarantee. No retained write-through move or arbitrary power-loss survival is
claimed. Path-based MoveFileExW semantics do not belong to this owner.

Local macOS runtime tests and Windows target compilation are distinct evidence.
Actual Windows runtime tests use the existing local-auth platform CI selection
(storage/auth/server/SDK tests and all-targets Clippy); no new job is required.
Until that runs, Windows runtime behavior remains unverified. Hosted elevated
runner evidence does not establish ordinary-user permissions independently.

Primary references: [FILE_RENAME_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info),
[SetFileInformationByHandle](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle),
[CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
[FlushFileBuffers](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers).
