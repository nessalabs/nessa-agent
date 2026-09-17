# Use local authentication

## Create owner access

Build with Rust 1.89 or newer, then initialize once:

```sh
cargo build -p nessa-server
target/debug/nessa auth init --local
pnpm server:run
```

The executable is `nessa`; the Rust package remains `nessa-server`. Install it on
PATH with `cargo install --path crates/nessa-server --bin nessa --locked`.
Run `nessa --help` for the command surface. `nessa server` starts the gateway;
calling `nessa` with no arguments displays help.

`auth init --local` is one-time offline bootstrap. It creates the gateway identity,
owner credential and panel credential before the gateway exists. The default
owner output is `<namespace>/auth/surfaces/nessa-cli.token`, used by the CLI.
An explicit `--owner-token-file /absolute/new.token` selects a different output;
then supply that path to online commands with `--credential-file`.
Do not run init for each browser login. Local registry commands still require
the gateway to be stopped. Cloud authentication is not implemented: `--cloud`
returns an error without provisioning local state.

With the gateway running, use a second terminal:

```sh
nessa doctor
nessa auth token | pbcopy  # macOS: paste into the browser sign-in form
```

`auth token` authenticates as a CLI surface over `/session` and requests a new
browser credential from the gateway. It never opens the live registry. Each
invocation creates a distinct member principal with only `server.read` and
`conversation.write` on the authenticated gateway and organization. Tokens have no expiry by default, capped by the CLI credential's expiry.
Use `nessa auth token --ttl 12h` (positive `s`, `m`, `h`, or `d` duration)
for temporary access, or `--no-expiry` to state the default explicitly. Only the secret goes to stdout;
request and credential IDs go to stderr. Issuance is never automatically retried.
If delivery is uncertain or output fails, inspect/revoke the recorded credential
using the administrative commands below before issuing again.

`doctor` checks configuration loading, private credential access, authenticated
connection and authorized health. It writes JSON to stdout and returns nonzero
on failure. Diagnostics do not include credentials. Online commands default to
local mode; `--local` is optional. They use the same `NESSA_*` namespace, stage,
host and port as the server, and cannot send local credentials to remote hosts.
Use `--credential-file /absolute/path` for an existing owner credential.

When specifying an output, use a new absolute path for the token file. The command writes it with mode
`0600` on Unix or a protected Windows ACL and prints only metadata. Treat the file as a password. Owner and ordinary credentials have no expiry by default. Set `--expires-at UNIX_SECONDS`
on offline commands, or `expiresAt` on issuance, when you want temporary access.
Explicit expiry must be in the future. There is no 24-hour or 30-day lifetime cap.

Initialization and serving must use the same `NESSA_DATA_DIR`, `NESSA_STAGE`, and
`NESSA_INSTANCE`. The data root defaults to `$HOME/.nessa` on Unix or `%USERPROFILE%\.nessa` on Windows; the registry is
`<root>/<stage>/instances/<instance>/auth/credentials.v1.json`. The `prod` stage
segment and an unset instance segment are omitted. Explicit instances isolate
production auth too. Data roots must be absolute and instance names safe path
segments.

The gateway serves authenticated WebSocket sessions at `/session`. The panel uses
this protocol and automatically loads its own credential through the native host.
`auth init` assigns it a distinct principal and private file at
`auth/surfaces/nessa-panel.token`, with all currently implemented permissions:
`server.read`, `conversation.write`, and `credential.manage`.
Use `auth init --local --owner-token-file /absolute/new.token --chat-grants server.read,conversation.write`
to restrict initial chat access. Client metadata never grants permissions.

## Mint a restricted token

First list credentials to obtain your organization and gateway IDs:

```sh
pnpm auth:cli list --url ws://127.0.0.1:7420 --credential-file "$HOME/nessa-owner.token"
```

Save this request as an absolute-path JSON file. Replace `ORG_ID`, `GATEWAY_ID`,
with your IDs. Omit `expiresAt` (or set it to `null`) for no expiry;
set future Unix seconds for a temporary token.
The CLI generates a `requestId` when omitted and prints it before issuance.
For retries across CLI runs, save that ID with the exact inputs, as shown below.
Use `--request-id ID` when retrying a revoke. The ID prevents duplicate changes;
it does not grant authority.

```json
{
  "requestId": "terminal-reader-1",
  "principal": { "id": "terminal-client", "kind": "integration" },
  "membership": {
    "id": "terminal-membership",
    "principalId": "terminal-client",
    "organizationId": "ORG_ID",
    "role": "member",
    "state": "active"
  },
  "grants": [{
    "action": "server.read",
    "resource": { "organizationId": "ORG_ID", "id": "GATEWAY_ID" }
  }]
}
```

```sh
pnpm auth:cli issue --url ws://127.0.0.1:7420 --credential-file "$HOME/nessa-owner.token" --input /absolute/issue.json --out /absolute/terminal.token
```

The output path must be new. The CLI writes the secret privately and prints its
credential metadata. This token can read server health; it cannot administer
credentials. Connect through `NessaClient.connect` with `profile: "product"` and
`auth: { credential }`, reading the credential from its protected file.

A retry with the same request returns `secretUnavailable: true` and the original
credential ID. Secrets are delivered only once. If delivery was lost, revoke
that credential and issue again with a new `requestId`. Changing the request
while reusing its request ID fails.

## Revoke a token

```sh
pnpm auth:cli revoke CREDENTIAL_ID --url ws://127.0.0.1:7420 --credential-file "$HOME/nessa-owner.token"
```

Revocation persists across restarts. Attached idle clients close on their next
state check (every second by default); subsequent commands are rejected. Already admitted
transport bytes cannot be recalled.

## Recover owner access

Stop the server, then use a new token path:

```sh
target/debug/nessa auth recover-owner --local --owner-token-file "$HOME/nessa-owner-next.token"
pnpm server:run
```

Recovery keeps the same gateway and organization and revokes previous
owner credentials. Other surface credentials retain their separate grants. It needs exclusive access to the registry, so it
fails while the server holds the lock. Never delete the registry to rotate a
token: that would discard its identity and revocation history.

## Verify the flow

```sh
cargo test -p nessa-auth -p nessa-server
pnpm auth:e2e
```

The end-to-end test uses a temporary data root. See the
[local authentication decision](../adr/done/0010-local-authentication.md) for storage,
authorization boundaries, and platform limits.

## Assign each surface its own credential

Stop the gateway, then provision or replace the credential assigned to a surface:

```sh
target/debug/nessa auth provision-surface --local --surface-id terminal --grants server.read
target/debug/nessa auth provision-surface --local --surface-id nessa-panel --grants server.read,conversation.write,credential.manage
```

Each surface has a distinct principal, membership, and token. Reprovisioning revokes
only that surface's prior credentials. Other surface names default to `server.read`;
`nessa-panel` defaults to the three implemented permissions above. Add `--expires-at`
for temporary access. This is an offline OS-authorized operation, never a capability
selected by remote `client.id` or `surface` metadata.

The Node SDK loads `auth/surfaces/<client.id>.token` automatically when `auth` is
omitted, using its stage and `NESSA_DATA_DIR` / `NESSA_INSTANCE` namespace. A custom
`CredentialSource` can use another host's private storage. The desktop panel injects
its native source. Browser callers must supply a source or explicit credential.
Loading fails clearly if the assigned file is missing or insecure. No owner token
is used as a fallback. Local file credentials are only loaded for numeric loopback
URLs (`127.0.0.1` or `::1`); `localhost` is not resolved or trusted for automatic
loading. Remote connections require `wss:` in every stage, including development.
URLs containing user information are rejected.

Surface principals have kind `integration`. Provisioning assigns `member` unless the
selected grants include `credential.manage`, which requires `admin`. Reprovisioning
replaces the surface membership role and revokes its previous credentials in one
commit. Owner recovery uses the registry's explicit `ownerMembershipId`, independent
of record order. The current registry contract requires this field; there is no
legacy fallback or automatic data migration.

Issuance requires at least one supported grant. The method list in `auth.session`
describes registered RPCs; it does not mean the caller may use them. Each protected
request still checks its exact grants and current membership.

Credential RPC failures distinguish `credential_conflict` (changed payload or
conflicting identity), `credential_capacity` (configured limit),
`credential_not_found`, and `credential_store_unavailable`. The first three are
command rejections and are not automatically retried. Mutations retain their
`requestId` in `NessaMutationError`; inspect its `cause` for the RPC error. Raise
capacity or change a conflicting command deliberately rather than blindly retrying.

## Configure limits without rebuilding

Create `config.json` beside the namespace's `auth` directory. For default local
development this is `$HOME/.nessa/dev/config.json`. For an explicit instance it is
`<root>/<stage>/instances/<instance>/config.json`; omit the `prod` stage segment.
Create this file with the same private permissions as credential files: current OS
user ownership, a single link, and mode `0600` on Unix or a private DACL on Windows.
Symlinks and unsafe existing files fail startup. On Unix, create a new config with
`(umask 077; cat > "$HOME/.nessa/dev/config.json")` and enter the JSON, then Ctrl-D.
The following values are the defaults:

```json
{
  "registry": {
    "maxCredentials": 1000,
    "maxRegistryBytes": 4194304,
    "maxReceipts": 2000
  },
  "session": {
    "handshakeTimeoutMs": 10000,
    "writeTimeoutMs": 5000,
    "currentStateIntervalMs": 1000
  }
}
```

Omitted fields use defaults. Restart the gateway after editing; offline auth
commands read the same settings on each invocation. Positive integers are required;
unknown fields, malformed files, and unrepresentable sizes or deadlines fail startup.
An absent file uses defaults. `maxReceipts` bounds each issue/revoke receipt collection;
revoked credentials still count toward `maxCredentials`. Raising capacity can unblock
issuance or owner recovery. Lowering limits below existing contents rejects opening
the registry and never removes records. Increasing write deadlines can increase how
long that connection waits for its own output. Other clients do not share its write
lock or wait for it. State checks bound idle revocation detection; command checks
remain mandatory. The handshake budget includes challenge delivery and is enforced
by one monotonic timer. The advertised Unix-second deadline rounds up from
millisecond wall time; expiry closes with retryable `handshake_timeout` (4006).
The SDK bounds its authentication wait by that deadline and its configured request
timeout.

## Operation admission and revocation

Each protected operation uses the latest committed credential/membership snapshot
available when it reaches authorization. If allowed, its handler and response may
finish after a concurrent revocation or expiry. New operations read again and fail
when they observe the change. A revoke response confirms publication of the new
state; it does not cancel previously admitted work. This also lets a self-revocation
return its successful acknowledgement.

Auth mutations are serialized within the store. Normal requests and network writes
have no global admission lock. Pending requests are checked when processed, and idle
sessions still close on invalidation. Future terminals or streams must authorize
meaningful actions/batches at checkpoints; a single admission must not grant an
unlimited stream of future operations.

Settings do not change platform private-file checks.

## Windows storage

Windows uses `%USERPROFILE%\.nessa` by default; `NESSA_DATA_DIR` still overrides the
root. The gateway, offline token commands, and desktop host share the
`nessa-local-storage` crate. It creates files and directories with an explicit,
protected DACL granting access only to the current process user's SID and
LocalSystem. Every open validates ownership and that DACL using the same handle
used for I/O. Inherited entries, other identities, null DACLs, hard-linked files,
and reparse points (including parent junctions) are rejected. Existing unsafe ACLs
are never silently repaired. The volume must support persistent ACLs (for example,
NTFS); FAT/exFAT storage is unsuitable.

The Node SDK and `auth:cli` use built-in Windows PowerShell to host a Win32 bridge.
It applies the same checks and passes paths and token bytes through stdin, never
command arguments. This requires Windows PowerShell with `Add-Type` available;
restricted environments that disable it fail closed. The desktop host does not
require PowerShell. Run the gateway and client as the same Windows account.

Registry writes flush a private temporary file before replacing the registry.
Windows replacement requests `MoveFileExW` write-through; it does not claim Unix
directory-fsync or universal power-loss guarantees. Token output is flushed before
success is reported. OS administrators and processes running as the same account
remain outside the separation provided by these files.

The full gateway/SDK lifecycle passes on Windows, Linux, and macOS in the
[platform CI run](https://github.com/nessalabs/nessa-agent/actions/runs/34085966204).
That run also covers native Windows credential loading and Node ACL rejection
checks. `.github/workflows/local-auth.yml` keeps these checks on subsequent changes;
consult the latest PR checks for the current revision's status.

The implementation follows Microsoft's [file security API](https://learn.microsoft.com/en-us/windows/win32/fileio/file-security-and-access-rights)
and [handle-based security inspection](https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-getsecurityinfo).

## Supported operating range and retention

The [ADR 0007 readiness review](../reviews/auth-api-readiness.md) measures the
default 1,000-credential registry, 64 local authenticated connections and eight
concurrent credential mutation callers on a reference Mac. It includes latency,
response sizes, memory, exact workload and limits on extrapolation. Run
`pnpm auth:measure` on deployment hardware before increasing these bounds.
These are measured bounds, not a connection quota or latency guarantee.

Retain revoked/expired records and mutation receipts for the registry lifetime.
There is no automatic cleanup. Leave capacity headroom for rotation and recovery;
raise configured limits deliberately if necessary, never delete registry records
to unblock a mutation. Keep the original request ID for explicit retries, including
revoke retries, to avoid consuming additional receipts. See the review for the
full retention decision and typed failure handling.

## Run Nessa in a browser

Browser sign-in uses an opaque server session cookie. HTTPS uses the
`__Host-nessa-session` cookie with `Secure`, `HttpOnly`, `SameSite=Strict`, and
host-only scope. Numeric loopback HTTP in dev/CI uses the separate
`nessa-local-session` cookie with the same attributes except `Secure`. Sessions
expire after 30 days of inactivity. Current credential expiry, revocation,
membership, grants, and policy are resolved independently on every admission.
While the browser is visible, it checks every five minutes and on focus;
the server extends the idle deadline at most once per hour and refreshes the cookie.
Refresh, browser restart, and gateway restart reconnect without entering a token again. Sign-out revokes that session on the server and
clears its cookie. Submitted access tokens are never stored in localStorage,
sessionStorage, Redux, or URLs. The server retains only the verified credential
ID, bound origin, and idle-lifetime evidence, not the submitted token or copied
identity claims. Sessions are bounded to 128 per gateway and persisted in the private namespace
file `auth/browser-sessions.jsonl`. Its append-only journal retains typed transition
causes (including credential revocation/expiry, inactive membership, identity
mismatch, and invalid credential state), honest
automatic or caller attribution, and before/after state; it contains no submitted access tokens.
Storage failure rejects the transition and makes the store unavailable until
restart; corrupt journals fail startup. Journal reads, writes, and flushes run on
the server's blocking pool and browser authentication deadlines cover the complete
storage and identity operation. The journal has a 64 MiB total bound, checked
before replay and before every append. Reaching the bound fails further session
transitions without discarding historical audit evidence or growing the file. To
rotate it, stop the gateway, move the journal to private archival storage, and
restart; browsers then sign in once to create a new journal. Do not truncate a live
journal or discard the archive.
Existing in-memory sessions need one sign-in after upgrading. Existing finite
tokens keep their original expiry; generate a new token for ongoing access.
Revocation and permission changes
continue to apply to every connection and operation.

For local development, run the gateway with `NESSA_STAGE=dev` (the default),
then run `pnpm dev` and open `http://127.0.0.1:1420`. No certificate is needed.
Both the gateway and frontend must use dev or CI stage for HTTP sign-in.
Only numeric loopback (`127.0.0.1` or `[::1]`) permits HTTP/WS; `localhost`, LAN
addresses, and alpha/prod stages do not. The server independently enforces this
policy; a frontend stage setting cannot enable it on a production gateway.

For HTTPS, start the current server build, then run the browser UI with a certificate your
browser trusts (for example, a localhost certificate issued by your development CA):

```sh
NESSA_BROWSER_TLS_CERT=/absolute/localhost.pem \
NESSA_BROWSER_TLS_KEY=/absolute/localhost-key.pem \
pnpm exec vite --host 127.0.0.1 --port 1443
```

Open `https://127.0.0.1:1443`. The certificate must cover `127.0.0.1`. Vite forwards
`/browser` HTTP and WebSocket requests to the local gateway on port 7420; set
`NESSA_BROWSER_GATEWAY_URL` to target a different test gateway. The browser uses
HTTPS/WSS on this URL, or HTTP/WS for loopback development; the proxy's upstream
hop is local loopback HTTP. Native
Tauri development retains its existing HTTP dev server. This does not add a
remote gateway deployment or install a trusted CA automatically.

Provision a dedicated browser surface token using the offline surface command
above with `--surface-id nessa-browser --grants server.read,conversation.write`.
Its token file is `auth/surfaces/nessa-browser.token` under the selected namespace.
Provisioning is an offline command: follow the existing gateway-stop requirement.
Enter that token once. Do not use the owner token for normal browser sessions.

Browser endpoints require a trusted loopback Origin and a custom request header;
the session is bound to the exact origin that signed in. No CORS access is enabled.
Cookie authentication is accepted only on `/browser/session`; native `/session`
continues to require explicit credential evidence. Sign-out closes all sockets
using that browser session on the next state check, without cancelling agents.
