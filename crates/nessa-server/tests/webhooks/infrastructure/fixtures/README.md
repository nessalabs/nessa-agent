# Isolated TLS recipient

`tls.rs` generates a fresh **test-only** CA and server certificate in memory
for each ephemeral loopback recipient. Keys are never written to disk. The
leaf has a `recipient.test` SAN, server-auth EKU and digital-signature key
usage. Its validity runs from one day before generation to seven days after
generation, below Apple's 825-day TLS leaf limit. There is no fixed fixture
expiry or dependency on an OpenSSL executable on CI hosts.

The original self-signed leaf lacked server-auth EKU and was valid for 100
years. The macOS platform verifier rejected it before HTTP. See
[Apple's TLS certificate requirements](https://support.apple.com/en-us/103769)
and the `rustls-platform-verifier` 0.7.0 test generator's matching constraints.

Tests retain the production client's TLS backend and platform verification,
adding only the per-recipient test CA and loopback DNS pin. Negative controls
require certificate errors for an untrusted CA and wrong hostname, with no
HTTP bytes received. No validation bypass, system trust-store change or
production private-address opt-in is provided. Production entry tests still
refuse loopback and other nonpublic DNS answers before connect.
