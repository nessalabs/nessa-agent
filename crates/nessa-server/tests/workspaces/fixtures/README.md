# Boat v1 response fixtures

Captured from authorized alpha tests against `https://boat.dev/api/v1` on
2026-10-27. `boat-v1-sandbox.json` is a real `GET /sandboxes/{id}` response;
`boat-v1-ssh-key.json` is a real `POST /sandboxes/{id}/sshkey` response. UUIDs,
VM identifiers, owner/name, URLs and network addresses are replaced with fixture
values (RFC 5737 / RFC 3849 addresses). The public SSH host key is preserved.
No API token, provider credential, private key, user file or cookie is included.

These fixtures protect the adapter's required provider response fields, including
the endpoint and host key needed for pinned SSH. They do not prove an agent turn;
the provider-backed E2E check separately exercises provisioning and inference.
