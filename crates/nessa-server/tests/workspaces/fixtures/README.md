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

## Environment admission

`boat-v1-environment.json` preserves the shape and agent-only settings observed
in a read-only personal-scope v1 environment response during the Daybreak
follow-up. The raw API envelope has `ok: true`, `type: environments.list` and
`environments`; the CLI's JSON projection omits the envelope metadata. In
particular, an environment's `access` is a ResourceAccess object, while a VM's
`access` is an enum string. The provider also reports four computed `effective`
credential flags; admission verifies agreement with the declared flags.
No environment contents, secret files, repository data or live identifiers were
copied into this fixture.

The provider's published schema and credential-profile documentation define
personal ResourceAccess, immutable environment versions and the explicit
`credentialProfileId: null` default profile. The currently returned `effective`
shape is additionally pinned by this observed fixture; extra fields or different
computed flags refuse admission.
