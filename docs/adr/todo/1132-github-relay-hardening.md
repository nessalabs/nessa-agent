# 1132 — GitHub relay storage and fair use

Issue: https://github.com/nessalabs/nessa-agent/issues/1132

The sign-in owner remains `GithubConnections`; the relay never owns another
GitHub login. Composition injects one OS credential-store entry per canonical
namespace and purpose. The inbox owns a separate random 256-bit database key;
signing out removes the login, not the key needed to read its encrypted audit.
macOS uses Keychain, Windows Credential Manager, Linux Secret Service. An
unavailable/locked service fails closed; there is no file-secret fallback.

The local database adapter uses SQLCipher with an OS-held raw key before schema
reads. It encrypts payloads, identities, cursors and audit pages, including the
rollback journal. Existing development plaintext is exported once into a private
encrypted temporary file and atomically replaced under a process-shared lock.
No encrypted database is opened without its key or reset on decryption failure.
This does not erase historical backups, snapshots or copies of old plaintext.
Cloudflare still sees webhook plaintext in memory and uses provider-managed at-rest
encryption: this is not end-to-end encryption.

| State / ordering | Required result / regression |
| --- | --- |
| Credential absent; save then restart | OS receipt precedes success; same identity restored |
| OS store locked or write fails | Unavailable; no plaintext fallback or sign-out claim |
| Legacy login; OS write/readback succeeds | Remove old file only after verified receipt |
| Interrupted login migration | Matching copies complete deletion; contradictory copies refuse |
| Two processes create inbox key | File lock serializes lookup/create; one key wins |
| Inbox exists but key missing/wrong | Refuse; preserve bytes, never create a replacement key |
| Legacy inbox export interrupted | Original remains authoritative until atomic replace |
| Correct key reopen / wrong key / changed cipher page | Read succeeds / refuses without replacement |
| Concurrent signed ingress reaches installation cap | Atomic admission admits only budget; duplicates stay idempotent |
| Repository identity churn | Retain at most 256 installation/repository pairs per installation and 4096 overall; preserve old gap markers |
| Invalid flood before GitHub verification | Native per-location limiter rejects before provider/DB work |
| Verified user exhausts read budget | 429 with retry delay before inbox/audit write; another user unaffected |
| Replayed watch ticket / concurrent upgrades | Four sockets per verified user per repository; capacity checked after alarm write |
| Out-of-order requests at a window boundary | Older timestamps cannot reset newer counters |
| Quiet connection / dropped wake / reconnect | Bounded lease, catch-up before and after subscribe; no payload from socket |
| Permission revoked during lease | Subsequent payload read rechecks GitHub and refuses |

Ingress quotas belong to signed installation identity, not a claimed sender or
HTTP header. User request budgets belong to GitHub-verified identity. Resource
limits are capacity controls, not a replacement for Cedar authorization. Native
edge limits are approximate and local to each Cloudflare location; Free request
allowance can still be exhausted by a distributed flood. Exceeding Free quotas
fails service rather than enabling billing. No paid resource is provisioned.

A 15-minute wake lease renewed at 12 minutes reduces quiet steady-state HTTP
requests from 2,880 to 360 per repository/day (two reads plus one upgrade).
The socket carries only a change hint; every payload page still requires current
GitHub access. Wake metadata revocation lag is bounded by the lease. A sleeping
machine catches up within retention, with gaps explicitly recorded beyond it.

Free development budgets: 2,500 retained events and 16 MiB per installation,
within the existing global 10,000 / 64 MiB ceiling; 120 reads/minute and 6,000/day
per verified user; at most 4 wake sockets per user/repository and 128 per room.
Budget identities are bounded to 4,096 users, with inactive rows swept in bounded
batches. Repository gap metadata retains installation/repository identities under
an admission cap, rather than discarding a watermark and silently replaying a gap.
An operator must provision a larger stage explicitly before these development
ceilings can support a large rollout. These limits do not make 10,000 users fit
Cloudflare's Free daily allowance.
