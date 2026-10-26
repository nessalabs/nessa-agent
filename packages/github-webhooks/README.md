# Nessa GitHub webhooks

Cloudflare Workers Free receiver and desktop relay for the nessalabs-owned
[Nessa Agent app](https://github.com/apps/nessa-agent), issue
[#1107](https://github.com/nessalabs/nessa-agent/issues/1107). Its ordering and
scope contract is [ADR 1107](../../docs/adr/todo/1107-github-webhook-receiver.md).
The [state page](../../docs/state/services/github-webhooks/README.md) explains
receipt outcomes. The existing [gateway admission primitive](../../docs/design/external-events.md)
accepts Nessa-specific signatures; this receiver does not invoke it or choose
a conversation from provider payloads.

## Module map

```text
composition/worker.ts  -> HTTP/signature/D1 adapters (runtime construction)
webhooks/infrastructure/http.ts -> application/receive.ts -> DeliveryStore port
webhooks/infrastructure/d1.ts   -> implements DeliveryStore with SQLite
webhooks/domain/delivery.ts    <- identity and bounded inbox values
migrations/                    -> atomic inbox and capacity accounting
tests/worker.test.ts           -> bundled Worker + local real D1 (Miniflare)
```

Arrows show imports or port implementation. The application imports no Cloudflare
runtime; composition supplies database, cryptography, clock, and reporting.

`POST /github/webhook` accepts signed JSON, stores it, then returns 202 (new) or
200 (identical duplicate). A reused ID with different bytes/event returns 409.
Malformed bodies/headers, wrong signatures, and payloads above one MiB return
400/401/413. A full or unavailable database returns 503. `GET /health` is process
liveness/configuration only; it does not query the database. The relay read and watch routes below are authenticated separately. No browser CORS
or user-token exchange is exposed.

## Live receiver

Endpoint: `https://nessa-github-webhooks.saurav-panthee.workers.dev/github/webhook`.
Workers Free was confirmed in the account dashboard. A real GitHub ping
`39bab46a-d05b-11f1-9d2e-c910672cafb6` returned 202 in 0.35 seconds and was
confirmed in D1. A real redelivery returned 200 with `duplicate` in 0.3 seconds.
All nine event selections below were read back after saving. The
[verification record](verification.json) and [GitHub receipt](evidence/github-redelivery.png)
retain the source checkpoint, checks, limits, and live evidence.
The app still needs installation on repositories selected by each account.

## Local verification

From the repository root:

```sh
pnpm install --filter @nessa/github-webhooks
pnpm --dir packages/github-webhooks typecheck
pnpm --dir packages/github-webhooks test
pnpm exec eslint packages/github-webhooks/src packages/github-webhooks/tests
pnpm exec prettier --check packages/github-webhooks
```

Tests use a fake secret and isolated databases; they do not call GitHub or use
production credentials. The package build emits an ignored `dist/worker.js`.

## Deployment

Use the existing Cloudflare account in `wrangler.jsonc`, with Workers **Free**.
Never upgrade as an automatic response to a capacity refusal. D1 free daily
limits can stop requests; the service's smaller inbox cap does not avoid them.

```sh
cd packages/github-webhooks
pnpm exec wrangler login --use-keyring --scopes account:read user:read workers:write workers_scripts:write d1:write
pnpm exec wrangler d1 migrations apply nessa-github-webhooks --remote
pnpm exec wrangler secret put GITHUB_WEBHOOK_SECRET
pnpm run deploy dev
```

The initial deployment used the signed-in dashboard because the least-scope
Wrangler token could create the database and secret but could not deploy scripts.
The editor contents were copied back and compared byte-for-byte with the tested
bundle before deployment. In Settings, bind `DB` to the configured D1 database,
pin compatibility to `2026-06-16`, enable logs, then deploy. In Domains, enable
only the production Worker URL. Future CLI deployment needs the additional
`workers_scripts:write` permission shown above.

Enter the dedicated random webhook secret at Wrangler's prompt. Enter the same
secret in GitHub App General > Webhook, set the deployed HTTPS URL followed by
`/github/webhook`, and enable Active. Do not put a secret in source, a command-line
argument, screenshots, or a PR. Wrangler's secret is an encrypted binding. The
GitHub App private key remains outside this repository and is not required here.

GitHub repository permissions: Contents, Issues and Pull requests read/write;
Actions and Checks read; Metadata mandatory read. Subscribe to Issues, Issue
comment, Pull request, Pull request review, Pull request review comment, Workflow
run, Workflow job, Check run, and Check suite. Verify an actual GitHub ping's 2xx
and its matching D1 delivery ID before reporting the connection as working.
Each user/organization must install the App on the repositories they choose.

## Operations and recovery

Inspect metadata, never publish raw private issue/comment payloads in logs:

```sh
pnpm exec wrangler d1 execute nessa-github-webhooks --remote --command 'SELECT deliveries, bytes FROM inbox_usage'
pnpm exec wrangler d1 execute nessa-github-webhooks --remote --command 'SELECT delivery_id, event, received_at FROM deliveries ORDER BY received_at DESC LIMIT 20'
```

The cloud inbox holds up to 10,000 deliveries / 64 MiB raw payload. Retention
has a seven-day target: a five-minute cron deletes at most 64 expired rows per
run, with deletion evidence and a per-repository expiry watermark in the same
transaction. Sweeps can lag the target. Full/quota-exhausted service refuses new
deliveries; it never upgrades the account. GitHub does not automatically retry
failed webhooks: inspect App Advanced > Recent deliveries and redeliver original
IDs after resolving the cause. A full free inbox is not lossless unlimited capture.

## User routing and offline catch-up

`GET /relay/v1/events?repository=NUMERIC_ID&after=SEQUENCE` requires a GitHub
bearer credential, `x-github-user-id` and `x-nessa-stage`. The identity header is
an expected identity, not proof: GitHub verifies the token's user and current
repository access on every page. Cedar evaluates those facts. Private repositories
require either GitHub-reported classic `repo` OAuth scope or proof that the
repository belongs to the active Nessa GitHub App installation accessible to the
user token. Cedar checks installation permissions for contents, issues, pull
requests, actions and checks (read or write). Metadata-only, unselected, wrong-App
and suspended installation facts do not authorize private payloads. Discovery is
bounded to 100 installations and eight repository-list pages per read; incomplete
proof refuses rather than assuming access. Fine-grained PATs without that proof
are not supported for private events. GitHub credentials are sent over HTTPS for
verification and are not stored by the relay.

Each page returns at most eight events and one MiB of source payload, an explicit
continuation flag, a next cursor and a fifteen-minute signed
watch ticket. `GET /relay/v1/watch` upgrades to WebSocket with `x-nessa-stage`
and `x-nessa-watch-ticket` headers. Tickets never go in URLs. SQLite Durable Objects
use hibernation; sockets carry only `changed`, without event data. Expiry closes
them. Every payload read still checks GitHub access. Authorization is a snapshot
at read admission; a read already admitted may finish after a revocation.

The native gateway imports into a SQLCipher-encrypted stage/instance inbox. It obtains
a ticket, watches, then catches up again to close the read/watch race. It renews
at 12 minutes (with a 15-minute wake-ticket lifetime), which also recovers lost wake-ups without polling GitHub event APIs.
Local event inserts and the cursor commit together. A lost response/crash replays
the page; competing consumers use a cursor and binding-generation check. Reconnect
preserves the cursor; a disconnect/reconnect generation fences in-flight old work.
Local delivery-ID deduplication also handles a cloud-expired event redelivered while
its original local copy remains. Neither store promises deduplication after erasure.

A cursor behind an expired event gets HTTP 410 and a durable local gap. It does
not silently advance. An explicit reconnect with `--accept-gap` records the local
owner's acceptance before advancing. Local storage retains seven days from import,
with a 10,000-event / 64 MiB payload cap and at most 16 repository/account bindings.
Cleanup runs in page transactions; without a consumer it can lag. Deletion and
connection/cursor changes keep bounded audit metadata (last 10,000 records per
log). The cloud access and deletion audit logs are likewise bounded, not archives.

From the corresponding development checkout/binary (the packaged app receives
its stage automatically from its host):

```sh
nessa github connect nessalabs/nessa-agent
nessa github events nessalabs/nessa-agent
nessa github disconnect nessalabs/nessa-agent
# Only after inspecting a reported catch-up gap:
nessa github connect nessalabs/nessa-agent --accept-gap
```

The serving gateway discovers connection changes within five seconds. `events`
reads the latest 100 events per matching local binding without network access;
it emits JSON on stdout, diagnostics on stderr. The local owner controls this
private inbox through OS file ownership, not a new unauthenticated HTTP route.
These events are data: this slice does not auto-create conversations or run tools.
Existing conversation command admission and Cedar remain their separate owners.
Settings › Connections › Accounts uses mainline's existing Nessa GitHub App
device sign-in. **Enable live GitHub events** opts in for its selected repositories
(up to 16). No separate `gh` authentication is required. **Refresh repositories**
applies a changed GitHub selection, retaining matching cursors. Missing history
is shown explicitly; **Continue with retained events** records acknowledgement
and retires the old follower generation. Disable clears this gateway's selection
and local inbox with an audit record. Disconnect serializes that cleanup before
the sign-in token is deleted; failed cleanup refuses sign-out. The watcher retires
removed generations within five seconds; already-returning pages cannot commit
against a removed generation. Neither action revokes the GitHub App installation
or deletes the shared cloud retention for other devices.

The CLI connection commands use the same stored Nessa sign-in. Automatic PR-to-chat
routing, agent actions and UI PR monitoring are a later capability.

## Stages and free-plan rollout

The desktop host already embeds `NESSA_BUNDLE_STAGE` and refuses a conflicting
runtime override. The relay reuses that selection. The native binary embeds
[`github-relays.json`](../../protocol/defaults/github-relays.json), so users do
not choose a stage or enter an endpoint. `dev` uses the existing workers.dev URL;
`alpha`, `ci` and `prod` are disabled and never fall back to dev. Shared wire and
storage bounds live in [the relay contract](../../protocol/defaults/github-relay-contract.json),
compiled into Rust and bundled into the Worker.

[`stage-deployments.json`](stage-deployments.json) selects each stage's Wrangler
file. To enable a rollout stage, provision an independent **free** Worker, D1,
SQLite Durable Object namespace and **separate GitHub App**, add its HTTPS origin,
app slug, numeric App ID and client ID to the public table, and add its Wrangler config path to
the deployment table. Each config supplies its own `RELAY_STAGE`, D1 ID and Worker
name; each deployment receives independent `GITHUB_WEBHOOK_SECRET` and
`RELAY_TICKET_SECRET` encrypted bindings. Deploy with `pnpm run deploy STAGE`.
The guard refuses incomplete configuration, shared Worker/database resources or
shared app identities/origins. It does not create production resources for you.
Never run ad-hoc `wrangler --env prod` against the dev-only config.

The current URL is **Nessa development**, even though Cloudflare calls a Worker's
live deployment slot "Production". No paid plan, Queue, KV or automatic paid
fallback is needed. Durable Objects use SQLite classes supported on Free. Workers
Free's 100,000 requests/day and D1/DO daily limits include idle renewals, reads,
audits and catch-up. One idle watched repository uses roughly 1,920 read requests
per day at the current renewal interval; this starter is not a 10,000-user free
service. Limit/refusal recovery may require operator redelivery of GitHub webhooks.

## Relay module map

```text
relay/domain/cursor.ts           shared bounds and wire identities
relay/application/read.ts        GitHub facts -> Cedar -> audit -> bounded page
relay/application/ports.ts       identity, policy, inbox, audit and ticket ports
relay/infrastructure/github.ts  fixed GitHub origin; credential never retained
relay/infrastructure/cedar/     validated schema/policy and bundled Cedar WASM
relay/infrastructure/d1.ts      page snapshot, retention and audit persistence
relay/infrastructure/tickets.ts stage-bound HMAC watch tickets
relay/infrastructure/wake-room.ts SQLite DO hibernating sockets; wake-up only
relay/infrastructure/http.ts    authenticated HTTP and watch adapter
```

The [native relay module](../../crates/nessa-server/src/github_relay/mod.rs),
[composition](../../crates/nessa-server/src/composition/github_relay.rs) and
[ADR 1117](../../docs/adr/todo/1117-github-relay.md) describe the local owner and
ordering matrix. The local receiver remains optional in unconfigured stages.

To disable ingestion, first deactivate the GitHub App webhook. Removing the
Worker stops HTTP service; removing D1 permanently removes its inbox and needs
separate explicit authorization. Keep the local secret until deliveries have
been disabled or a deliberate rotation has finished on both services.


## Storage and fair use hardening (#1132)

GitHub sign-in and the separate random inbox encryption key use native OS secret
storage (macOS Keychain, Windows Credential Manager, Linux Secret Service).
No plaintext secret-file fallback is provided. Keys already acquired remain in process memory until the gateway exits. Linux desktop/headless users need
an unlocked Secret Service session; a locked or unavailable service refuses access.
The inbox is encrypted in full with SQLCipher, including identities, payloads,
cursors, audits and journal pages. Sign-out deletes the login, while retaining the
separate inbox key for encrypted cleanup evidence. Losing that key makes the
existing inbox unreadable. Backing up a file alone does not back up its OS key.

Before upgrading the existing dev inbox, stop old gateway processes. The upgrade
verifies a new encrypted copy and atomically replaces the original. Existing login
JSON is removed only after OS write/readback. Historical plaintext backups and
filesystem snapshots are not erased by migration. Cloudflare receives plaintext
webhooks over TLS and provides managed encryption at rest; this is not E2EE.

The Free development relay retains at most 2,500 events / 16 MiB per signed
installation, within the existing 10,000-event / 64-MiB total. A verified GitHub
user gets 120 page attempts/minute and 6,000/day. These budgets are atomic;
refused payload reads return 429 and Retry-After. Native clients back off five
minutes on throttling. Edge limits run before provider calls: 120 relay requests
per IP or credential/minute, and 600 webhook attempts/IP/minute. Edge counters are
approximate per Cloudflare location, not a global denial-of-service guarantee.

There are at most four sockets/user/repository and 128/room. User budget metadata
is bounded to 4,096 identities and swept after inactivity. Repository gap metadata
has admission bounds of 256 installation/repository pairs per installation, 4,096
overall; old gap watermarks are preserved. Storage quotas reject new work, without
evicting accepted data. GitHub does not automatically retry refused deliveries.

A virtual quiet-day regression counts **240 page reads + 120 upgrades = 360 HTTP
requests/repository/day**, down from 2,880 (87.5%). This excludes reconnects,
events, DO messages/alarms and other service usage. Every payload read still
rechecks GitHub permissions; only wake hints can outlive revocation, for at most
the remaining ticket lease. The Free daily allowance is still shared and cannot
support 10,000 busy users. No paid resource or automatic upgrade is configured.
Invocation logs are disabled; sampled application logs contain delivery metadata
and typed outcomes, not credentials or event bodies.

See [ADR 1132](../../docs/adr/todo/1132-github-relay-hardening.md) for orderings,
security boundaries and the review record.

Expiry uses covering indexes and an audit primary-key cutoff; the full-capacity query-plan regression rejects idle full scans. Linux Secret Service sessions use prompt timeout zero, including lock-after-lookup races. OS credential identities use the canonical namespace, so filesystem aliases share a key.

The [hardening verification](hardening-verification.json) and
[adversarial review](evidence/hardening-review.md) record the source checkpoint,
security boundaries and current Free development deployment. Metadata admission
uses trigger-maintained counters; cleanup accounting is keyed by installation,
with execution regressions that include trigger work.
