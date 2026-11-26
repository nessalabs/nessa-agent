# Static app packages

The headless verifier accepts a JSON manifest and a map of held asset bytes.
It checks that the manifest describes exactly those bytes and returns one
owned record. Callers can read the HTML entrypoint and the other assets
directly from that record.

The provisional version 1 manifest uses `schemaVersion` (1), `appId`,
`packageVersion`, `entrypoint`, optional `icon`, `assets`, and
`requestedCapabilities`. Each asset declares `path`, `sizeBytes`, and a
lowercase hexadecimal `sha256`. The entrypoint names a declared `.html` asset
and the icon, when present, names a declared asset. Paths use forward slashes
and have up to 240 bytes, eight components, and 64 bytes per component.
Components start with a lowercase ASCII letter or digit and use lowercase
letters, digits, dots, underscores and hyphens. Components cannot end in a dot
or use a Windows device name.

The parser rejects unknown fields and bounds the manifest JSON to 64 KiB and
128 asset declarations. The portable verifier accepts bytes that the caller
already holds. A separate Product transport would bound these bytes before
delivery. The five possible capability declarations are `list`, `read`,
`create`, `subscribe`, and `stop`, each at most once. Declarations confer no
authority. Integrity checks confirm correspondence with the manifest.

The optional `disposable-store` feature adds a private SQLite adapter for
local fixtures. An install writes the manifest and assets in one transaction.
Installing an existing app identity returns `AlreadyInstalled`. A load
reads a single database snapshot and checks the stored identity, manifest,
and asset bytes before returning an owned record. The adapter has no mounted
Product route or authorization.

Inventory reads app IDs and versions from one snapshot. An empty list means
the store has no installed records. Unreadable or invalid records return an
error. Asset integrity is still checked on load. Removal matches the exact
canonical ID and deletes its manifest and assets in one transaction.

| Operation | Stored outcome |
| --- | --- |
| Remove an installed ID | That app and its assets are gone. Other apps keep their original records. |
| Remove an absent ID | Returns `NotInstalled`. |
| Removing the manifest fails after deleting its assets | The transaction rolls back the asset deletion. |
