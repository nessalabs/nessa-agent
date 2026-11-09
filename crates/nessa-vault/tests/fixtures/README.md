# Public format fixture

`format-one.enc` contains only `example-secret = public fixture value`, encrypted
with the public passphrase `public vault format verification words`. It is not a
credential. The golden read prevents a KDF-profile or framing change from silently
making existing format-one vaults unreadable. Regeneration requires an explicit
format/migration decision, not a routine test update.


`legacy-weak.enc` is a public synthetic format-one empty map encrypted with
`aaaaaaaaaaaa`, fixed salt `[0x42; 16]`, fixed nonce `[0x24; 24]`, and the original
64MiB/3/1 Argon2id profile. It proves setup hardening does not strand previously
accepted twelve-character vaults. It contains no real secret and is never used
for live setup. Its fixed nonce is for this one immutable fixture only.
