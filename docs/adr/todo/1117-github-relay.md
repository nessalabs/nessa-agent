# 1117: GitHub relay with local catch-up

Issue: https://github.com/nessalabs/nessa-agent/issues/1117. Builds on #1107 / #1111.

## Decision

The packaged host already embeds NESSA_BUNDLE_STAGE and rejects a different
runtime stage. Reuse that owner. A checked-in public stage table selects relay
origin and GitHub app identity; dev uses the existing workers.dev hostname.
Alpha, CI and prod are disabled until independently provisioned. No fallback.
Each deployed stage owns its D1, webhook secret and Durable Object namespace.
GitHub Apps have one webhook URL: production must use a separate GitHub App,
client identity and credentials, so rollout cannot divert development webhooks.

A desktop authenticates using its local GitHub CLI credential, transmitted only
over TLS to the configured relay, never persisted or logged there. GitHub verifies
the user and repository access on every catch-up request. Cedar decides whether
the verified user may read that repository. Repository identity, not name, keys
retained events. An installation is not treated as a single user's ownership.
The local binding records the verified GitHub user identity; changing gh accounts
requires reconnecting rather than reassigning the old inbox.

WebSocket notifications carry only a wake-up, never payload or repository/event
metadata. Short-lived signed tickets bind stage, repository and GitHub identity.
The local consumer connects before catching up, persists each page and cursor
atomically, and fetches again on wake-up/reconnect. No GitHub event polling.
The cloud retains seven days within the existing 10,000 / 64MiB cap. Expiry is
bounded and audited; a cursor behind the per-repository expiry watermark gets an
explicit gap. Capacity/free quota refusal is visible; GitHub does not retry failures.
No exactly-once tool effects or automatic conversation submission is claimed.

## Ordering matrix (implementation contract)

| ID | Ordering | Required outcome |
|---|---|---|
| R1 | Invalid credential / revoked repo | Refuse before payload disclosure |
| R2 | User A requests repository B without access | Cedar denies; no events |
| R3 | Repo name changes / name reused | Route by GitHub numeric identity |
| R4 | Lost webhook answer / repeated delivery | One retained event; original ID |
| R5 | Receive while desktop offline | Retained page on next authenticated reconnect |
| R6 | Local write fails / process dies before cursor commit | Same page replays; cursor unchanged |
| R7 | Local commit succeeds then connection drops | Saved cursor resumes without duplicates |
| R8 | Event arrives between watch and catch-up | Wake-up coalesces; catch-up drains pages |
| R9 | Wake-up is lost after cloud commit | Bounded reconnect re-reads committed inbox |
| R10 | Retention removes events after saved cursor | Explicit gap, never silent success |
| R11 | Capacity or daily free quota exhausted | Visible refusal, no paid upgrade |
| R12 | Wrong stage / unconfigured stage | Refuse; no dev fallback |
| R13 | gh identity changes after local binding | Stop; no old-account events in new inbox |
| R14 | Watch ticket expired/tampered/wrong stage | Refuse before socket registration |
| R15 | Cloud/local cleanup | Bounded transaction records IDs, cause and system initiator |
| R16 | Two local consumers / retries | SQLite transaction and cursor compare keep one history |
| R19 | Watch alarm persistence fails before socket admission | No socket registered without a confirmed expiry alarm |
| R18 | Several maximum-size events are pending | Byte-bounded page avoids D1 row limits; explicit hasMore drains all pages |
| R17 | Disconnect then reconnect while an old read is in flight | New binding generation fences the old read and cleanup |

## Consequences

Cloudflare remains a small provider inbox. Nessa stores received events locally;
code and AI execution remain local. A seven-day inbox is bounded catch-up, not
an indefinite event archive. The authenticated local events command exposes the
owner's private inbox to their local workspace; payloads do not grant tool authority
or select a conversation. Existing command admission/Cedar remains the owner of
any future event-to-turn binding. The first connection requires gh authenticated
on that machine; missing gh is an explicit disconnected state.
