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

Run `cargo test -p nessa-local-storage` and
`cargo clippy -p nessa-local-storage --all-targets -- -D warnings` on each supported
platform. The platform CI workflow also exercises the full auth registry and SDK.
