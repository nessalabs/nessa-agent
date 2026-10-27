# Boat workspace verification

`boat-e2e.mjs` is an opt-in test against a real Boat account. It provisions
billable disposable VMs using the account's connected Claude subscription. Run
from the repository root after building the gateway:

```sh
cargo build -p nessa-server --bin nessa
node scripts/workspaces/boat-e2e.mjs --token-file /private/path/boat-token --evidence /tmp/nessa-boat-evidence
```

The token file contains only a Boat API token, with access restricted to its
owner. Never paste it into command arguments or commit it. Connect Claude in
Boat first. Nessa's release worker image must match the gateway's lease protocol;
a source build with a different protocol refuses setup until its matching worker
image is published. The image contains no user's credentials.

The test starts a temporary gateway. Its Local harness is explicitly scripted;
cloud harnesses execute the real prebuilt Claude adapter with Boat's subscription
credentials. It checks independent VM assignments, inference, a workspace file
across stop/resume and gateway restart, an approved read of one temporary local
log, removal and session-close revocation of that connection, and Nessa coordinator-to-worker delegation.
It also reads real provider lifetime receipts: a finite channel override on
creation, explicit unlimited on archived resume, default inheritance after reset,
and an unchanged deadline on an
already-running VM after a settings edit. Settings use the isolated gateway's
ordinary configuration file; the token's encrypted storage stays independent.
Only `scripts/mcp-test-server/boat-local-files.mjs` can read that test log. No real
user folders are shared. Its disposable VM writer uses full access inside its own
VM; local-log and delegation permissions use one-time approvals of only those
fixture tools. Unexpected permission requests fail the test. Configuration removal
keeps already-open MCP sessions by the existing contract, so the revocation check
closes/reopens the reader before asking again.

Evidence includes only check names, test conversation/VM identities and cleanup
receipts. The test records its owned VM IDs before requesting deletion, waits for
provider-confirmed absence, and reports unconfirmed cleanup explicitly. Inspect
`cleanup.json` and remove any remaining **listed test VMs** in Boat if the process
was interrupted. Do not delete unrelated account VMs. This command is intentionally
excluded from default CI: subscription access and cloud spending are required.

Sanitized real provider response fixtures live under
`crates/nessa-server/tests/workspaces/fixtures`; allocation interruption and
concurrency checks run in the normal Rust suite. Channel placement is covered by
the desktop conversation stress test's `channel` step.

The missing-VM case restarts the isolated gateway after provider-confirmed
absence, then requires `environment_unavailable` on the saved assignment. This
checks durable recovery with a fresh connection. A surviving live connection may
instead retain an earlier unconfirmed cleanup after external deletion; the runner
does not reset that live state or infer cleanup from the provider's deletion reply.
