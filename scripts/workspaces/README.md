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
It uploads an original text file through the authenticated attachment API, asks
the cloud agent to read a marker absent from the prompt and save a workspace copy,
then requires that copy after the same VM resumes. This combines stored-input
delivery with the selected lifetime and workspace identity; no host path is sent.
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

Delegation permits one child-creation approval per turn; the existing assertion
still requires exactly four distinct VMs. The allowance is reserved before the
answer is sent, because an interrupted answer may already have executed.
This bounds reviewed coordinator approvals, not account-wide allocation or spend.
The existing Full-access writer can execute tools without a review; monitor the
run's actual provider resources and cumulative usage against its operating limits.

| Review delivery | Fixture action |
| --- | --- |
| First child creation | Reserve the allowance, then answer once |
| Same review repeated | Do not answer again |
| Different second child creation, including while an answer is pending or after it fails | Fail before answering |
| Delegated send-message or the selected local-log fixture | Keep the existing narrow one-time approval |
| Any other tool | Fail before answering |

Final cleanup closes conversations, stops the gateway and reads its final
allocation receipts before the gateway removes its private directory. VMs assigned
during close or shutdown remain owned and are deleted through the same
provider-confirmed-absence check. Provisioning without an owned VM receipt, an
unreadable ledger, or an unconfirmed/nonzero gateway shutdown remains a failure;
an unrelated VM named only in a refused receipt does not authorize deletion.
`cleanup.test.mjs` covers those orderings, and `local-gateway.test.mjs` checks the
final inspection against a real disposable local process.

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
