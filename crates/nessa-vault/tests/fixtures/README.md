# Public format fixture

`format-one.enc` contains only `example-secret = public fixture value`, encrypted
with the public passphrase `public vault format verification words`. It is not a
credential. The golden read prevents a KDF-profile or framing change from silently
making existing format-one vaults unreadable. Regeneration requires an explicit
format/migration decision, not a routine test update.
