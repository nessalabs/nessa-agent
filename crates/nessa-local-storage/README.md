# Private native local storage

Shared filesystem mechanics for the gateway's auth adapter and desktop credential
loading. This crate has no application authorization rules, provider clients, or
process-wide handles.

Unix checks current ownership, owner-only files, link count, and no-follow opens.
Windows supplies a protected DACL at creation (current user SID + LocalSystem),
then checks the DACL, owner, persistent-ACL volume support, link count and reparse
attributes through the same file handle used for I/O. Windows paths must be local
absolute drive paths; alternate streams and device namespaces are rejected.
Temporary files are private before data is written. Callers flush payloads before path-based replacement. Windows replacement uses
`FileRenameInfoEx` with POSIX semantics so delete-sharing readers retain the old
file while new opens observe the replacement. Retained Windows replacement also
flushes the file before and after handle rename; Unix callers also sync the containing directory.

[Retained-directory publication design](../../docs/design/retained-directory-publication.md)
owns the #324 ordering table and platform evidence boundaries.

## Module map

| Path | Responsibility |
| --- | --- |
| `src/lib.rs` | Crate documentation, module declarations, path-based private-storage API, and the exact reservation-name syntax classifier. |
| `src/physical_operation.rs` | Default-off `physical-operation` capability: one-slot Worker admission, synchronous Admission submission and private Job captured cleanup; adapter-owned typed mappings remain with consumers. |
| `src/physical_operation/tests.rs` | Central admission, executor, cancellation and ordinary call/capture fault contracts. |
| `tests/support/physical_operation.rs` | Source-included cfg(test)-only actual adapter capture observations and OS watchdog fixtures; no execution/admission policy. |
| `src/retained_directory.rs` | `PrivateDirectory`, retained root/child acquisition, native entry snapshots, origin-bound temporary files, and typed publication evidence. |
| `src/unix/retained_directory.rs` | Retained root and child directory descriptors with cloned captured ancestry, safe absolute locator ancestry, independent `openat(".")` enumeration cursors, identity checks, exclusive publication, atomic replacement, cleanup, and directory sync. |
| `src/windows.rs` | Path-based private Win32 storage and the shared handle rename encoder: exclusive publication or POSIX replacement that retains delete-sharing readers. |
| `src/windows/retained_directory.rs` | Top-down non-delete-sharing root/child directory handles, retained ancestry clones, transient identity probes, handle enumeration, delegation to the shared rename encoder, and handle disposition cleanup. |
| `tests/retained_directory.rs` | Cross-platform root/child acquisition, authority, enumeration, publication, cleanup, replacement, and native Windows handle-lifetime coverage. |
| `tests/windows_replacement.rs` | Native Windows path-based replacement with retained readers, denied delete sharing, and read-only destinations (#874). |

`PrivateDirectory` is acquired once beneath a trusted absolute root. Every file
operation accepts one native single-component name and stays on that authority.
Enumeration returns native names and no-follow type snapshots; it is non-atomic,
and each iterator owns an independent native cursor. Consumers that coordinate
records with a stable lock open a fresh lock handle for each lock attempt and use
`named_file_is` against both the originally acquired lock and the current handle.
They never recreate a missing or replaced lock through this API.

`PrivateDirectory::open_root` retains an existing private absolute root, including
an empty directory. `open_directory` acquires one existing private child through
that retained authority and keeps its captured ancestry alive after the parent
handle is dropped. Neither acquisition creates files or directories, repairs
permissions, nor requests directory write access. The existing `open_beneath`
contract still requires a nonempty relative path and retains its Windows final
directory publication access. Ancestry above a caller-trusted root remains the
caller's responsibility; `open_root` accepts a private root beneath a publicly
writable parent under the same trust assumption as `open_beneath`.
Root acquisition normalizes lexical separators and `.` components before its
no-follow open, so a trailing separator or `/.` cannot turn a root symlink into
an accepted directory (`retained_root_rejects_symlinks_with_trailing_root_spellings`).
This normalization does not resolve parent components or change ancestor trust.
`retained_empty_root_refuses_reads_after_rename_and_private_replacement` and
`retained_child_keeps_original_root_and_child_bindings_after_parent_drop` check
Unix replacement refusal; the Windows root/child lifetime tests check name
pinning until the final retained handle drops.

Unix callers that receive an OS-resolved application directory may instead use
`create_private_directory_path` and `PrivateDirectory::open_path`. Those APIs walk
from `/` with no-follow directory handles, permit root/current-user locator
ancestors only when group and other users cannot write them, require the final
directory to be current-user-owned and private, and retain every traversed identity
for later binding checks.

`publish_new` refuses occupied destinations; `replace` replaces a name or publishes
an absent name. Both results keep the exact destination,
opaque native identity, and open file handle. If rename succeeds but a later flush,
binding check, or directory sync fails, the typed failure retains that published
fact and handle so the consumer can reconcile it. Cleanup is disarmed immediately
after rename; published files are never deleted as temporary reservations. Before
rename, cleanup is origin-bound and any cleanup error is reported separately.
`Drop` is best-effort and makes no successful-cleanup claim.
`is_private_temporary_name` recognizes only the exact native name grammar used
for reservations. A match is syntax, not provenance or authority; consumers may
preserve such abandoned regular files but must not infer that they can open,
publish, or remove them.

Binding checks are acknowledgement checkpoints, not continuous attachment. Unix
directory descriptors prevent ancestor redirection, while a caller's stable lock
excludes same-UID mutation in the residual check/effect interval. Without that
cooperation, a mutation precisely inside the final name-check/rename or
name-check/unlink interval is outside this API's guarantee. Windows retains the
real directory chain without `FILE_SHARE_DELETE`, so directory replacement is
prevented while the authority lives. Mutation-capable file handles use the same
name-pinning policy; transient identity probes and read-only opens share deletion
so they can inspect a reservation or destination while its publication handle
remains open. `PrivateDirectory::sync` syncs the retained directory on Unix. On
Windows it only revalidates binding: Windows has no directory fsync equivalent,
and success does not claim directory-entry durability or survival of arbitrary
power loss. Retained Windows publication uses the shared handle rename encoder
through the reservation handle with the fully validated extended destination path, and flushes
that file before and after rename. The retained root and intermediate handles pin
that path from the caller-trusted root down. Ancestors above the root, an upstream
reparse point, and mutable drive mappings remain part of the caller's root-trust
assumption. Publication does not claim a directory-fsync equivalent.

Run `cargo test -p nessa-local-storage` and
`cargo clippy -p nessa-local-storage --all-targets -- -D warnings` on each supported
platform. Security-sensitive stores use the beneath-root operations so every
intermediate directory and final publication stays bound to verified directory
handles. The platform CI workflow also exercises the full auth registry and SDK.

## Optional physical operations

The `physical-operation` feature supplies `Worker`, `Admission` and `Interrupted`
for native async persistence adapters. It adds Tokio only when selected. A Worker
is constructed without a runtime and owns one instance slot. Admission captures
the originating executor before waiting. Synchronous `Admission::submit` transfers
the operation capture and slot to that executor before returning its awaitable;
dropping an unpolled awaitable therefore detaches an already-owned job. Executor
entry unwind returns typed interruption while Tokio may still own a queued job;
its slot remains owned until actual operation/capture cleanup. See the canonical
submission ordering and progress limitation below.

The [physical persistence ordering table](../../docs/design/bounded-physical-persistence.md)
owns lifecycle cases and exclusions. Actual connection/directory/audit state and
method-specific typed outcomes remain in SDK/server adapters. This capability
does not choose revisions, OAuth generations or settlement authority.

Verify both `cargo test -p nessa-local-storage` and
`cargo test -p nessa-local-storage --features physical-operation`. The existing
combined six-package CI compile selects this feature through SDK/server and runs
the central mechanic tests in its local-storage test binary.
