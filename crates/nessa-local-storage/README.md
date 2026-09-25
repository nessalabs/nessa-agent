# Private native local storage

Shared filesystem mechanics for the gateway's auth adapter and desktop credential
loading. This crate has no application authorization rules, provider clients, or
process-wide handles.

Unix checks current ownership, owner-only files, link count, and no-follow opens.
Windows supplies a protected DACL at creation (current user SID + LocalSystem),
then checks the DACL, owner, persistent-ACL volume support, link count and reparse
attributes through the same file handle used for I/O. Windows paths must be local
absolute drive paths; alternate streams and device namespaces are rejected.
Temporary files are private before data is written. Windows replacements request
write-through moves after file flush; Unix callers sync the containing directory.

## Module map

| Path | Responsibility |
| --- | --- |
| `src/lib.rs` | Crate documentation, module declarations, path-based private-storage API, and the exact reservation-name syntax classifier. |
| `src/retained_directory.rs` | `PrivateDirectory`, native entry snapshots, origin-bound temporary files, and typed publication evidence. |
| `src/unix/retained_directory.rs` | Retained directory descriptors through private roots or safe absolute locator ancestry, independent `openat(".")` enumeration cursors, identity checks, exclusive rename, cleanup, and directory sync. |
| `src/windows/retained_directory.rs` | Top-down non-delete-sharing directory handles, transient identity probes, handle enumeration, `FileRenameInfo` publication, and handle disposition cleanup. |
| `tests/retained_directory.rs` | Cross-platform authority, enumeration, publication, cleanup, replacement, and native Windows handle-lifetime coverage. |

`PrivateDirectory` is acquired once beneath a trusted absolute root. Every file
operation accepts one native single-component name and stays on that authority.
Enumeration returns native names and no-follow type snapshots; it is non-atomic,
and each iterator owns an independent native cursor. Consumers that coordinate
records with a stable lock open a fresh lock handle for each lock attempt and use
`named_file_is` against both the originally acquired lock and the current handle.
They never recreate a missing or replaced lock through this API.

Unix callers that receive an OS-resolved application directory may instead use
`create_private_directory_path` and `PrivateDirectory::open_path`. Those APIs walk
from `/` with no-follow directory handles, permit root/current-user locator
ancestors only when group and other users cannot write them, require the final
directory to be current-user-owned and private, and retain every traversed identity
for later binding checks.

Publication never replaces a destination. Its result keeps the exact destination,
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
power loss. Retained Windows publication uses `FileRenameInfo` through the
reservation handle with the fully validated extended destination path, and flushes
that file before and after rename. The retained root and intermediate handles pin
that path from the caller-trusted root down. Ancestors above the root, an upstream
reparse point, and mutable drive mappings remain part of the caller's root-trust
assumption. Publication does not claim a directory-fsync equivalent.

Run `cargo test -p nessa-local-storage` and
`cargo clippy -p nessa-local-storage --all-targets -- -D warnings` on each supported
platform. Security-sensitive stores use the beneath-root operations so every
intermediate directory and final publication stays bound to verified directory
handles. The platform CI workflow also exercises the full auth registry and SDK.
