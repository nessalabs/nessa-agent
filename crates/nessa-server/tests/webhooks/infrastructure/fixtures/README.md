# Isolated TLS recipient

`recipient.der` and `recipient-key.der` are a self-signed **test-only** TLS
certificate and PKCS#8 private key for `recipient.test`. The fixtures use an
ephemeral loopback listener. No production adapter trusts this certificate or
offers a private-address opt-in. These are not account or webhook credentials.

The certificate expires in 2126. The HTTP fixture tests the common request
writer with an explicit test trust root and loopback DNS pin. Separately, the
production entry's destination/DNS refusal and concurrency/deadline checks run
through `WebhookHttp::post`; it refuses loopback addresses.
