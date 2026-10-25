# Nessa GitHub webhooks

Standalone Cloudflare Workers Free receiver for the nessalabs-owned
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
liveness/configuration only; it does not query the database. All read APIs are
absent. No CORS, desktop routing, or user-token exchange is implemented here.

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
pnpm run deploy
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

The inbox retains up to 10,000 deliveries / 64 MiB of raw payload, without expiry
or eviction. A full inbox returns 503 for new IDs while accepting known identical
duplicates. Export to a protected file before any deliberate cleanup, verify the
export, and preserve deletion evidence with the operator's identity and reason.
Deletion is an explicit separate operator action. Never silently delete data to
make a failing request succeed. Accounting triggers follow explicit row deletes.

GitHub does **not** automatically retry failed webhook deliveries. In the App's
Advanced > Recent deliveries, inspect failures and redeliver after fixing the
cause. An ambiguous/lost response can be safely redelivered under its original
UUID. Before adding a consumer, define retention, authenticated ownership, and
automatic recovery from missed deliveries; this receiver alone does not deliver
events into desktop conversations.

To disable ingestion, first deactivate the GitHub App webhook. Removing the
Worker stops HTTP service; removing D1 permanently removes its inbox and needs
separate explicit authorization. Keep the local secret until deliveries have
been disabled or a deliberate rotation has finished on both services.
