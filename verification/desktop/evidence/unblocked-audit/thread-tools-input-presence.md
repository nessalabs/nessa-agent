# Tool argument presence

The published input schemas allow omission of optional scalar fields, but do
not allow JSON null. Ordinary Serde `Option<T>` decoded both as `None`, so an
explicit null executionId selected Stop's default active execution. The same
normalization also affected optional identities, configuration and event-read
options.

One adapter decoder now requires every supplied optional scalar to deserialize
as its declared `T`; Serde's field default applies only to omission. All four
tool argument types use that decoder for their optional scalar fields. Existing
typed required fields and SendMode retain their own decoding. No second schema
validator, source authority, permission comparison or runtime is introduced.

The schema-driven argument matrix covers every published field: a valid scalar
decodes, null returns invalid_request, omission is accepted only for optional
fields. Cases come from the published tool schemas, so a new optional field
without presence-preserving decoding is caught by that matrix.

The actual MCP relay regression holds a worker execution active, sends
executionId:null and asserts invalid_request, unchanged source audit count,
unchanged provider cancellation count and the same authoritative active
execution. Subsequent valid steering, omitted-execution Stop and explicit retry
retain the original exact-execution and single-cancellation assertions.

All nine agent-tool tests and all thirteen adapter/subscription tests pass.
Reverting only the shared decoder to `Option::<T>::deserialize` makes that same
relay case return a successful cancelled receipt for request null-stop; the
unchanged invalid_request assertion fails (exit101). The hunk was asserted before
the run. Restoring fixed source with fresh mtime and byte equality gives exit0
for the relay and all thirteen adapter/subscription tests. No assertion or
timeout was weakened. The design ordering table and decoder ownership explain
the distinction between omission and supplied values.
