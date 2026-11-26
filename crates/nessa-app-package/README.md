# Static app packages

The headless verifier accepts a JSON manifest and a map of held asset bytes.
It checks that the manifest describes exactly those bytes and returns one
owned record. Callers can read the HTML entrypoint and the other assets
directly from that record.

The local version 1 contract has four fields: `schemaVersion` (1), `appId`
(the canonical extension app identity), `entrypoint` (the exact path of a
declared `.html` file), and `assets` (an array of declarations). Each asset
declares `path`, `byteCount` and a lowercase hexadecimal `sha256`.
Unknown fields are rejected. Path segments start with a lowercase ASCII
letter or digit and use lowercase letters, digits, dots, underscores and
hyphens. Segments cannot end in a dot or use a Windows device name.
Use forward slashes between segments.

The portable profile permits up to 128 assets and 64 KiB of total asset
bytes. The manifest has a separate 64 KiB decode limit. This contract is
available in the headless library. A Product transport envelope and mounting
are separate concerns. Integrity establishes that the held bytes match the
manifest. Publisher trust and permissions require separate authority.
