# GitHub connect

A person signs Nessa in to GitHub once, in Settings › Connections ›
Accounts, and agents can request bounded repository reads through the gateway.
The sign-in goes through the **Nessa GitHub App**
(`protocol/defaults/github-app.json`: client id `Iv23linAB5VSgOF5PvKi`, slug
`nessa-agent`) and GitHub's device flow, so nothing needs a client secret and
no browser has to come back to the gateway.

## Who owns what

The gateway owns the sign-in, because agents run where the gateway runs: a
phone or a second window sees the same one. The owner is
`github::application::GithubConnections`; nothing else changes it.

| Fact | Owner | Kept in |
| --- | --- | --- |
| Which app, and GitHub's addresses | `github/infrastructure/app.rs` | `protocol/defaults/github-app.json`, or the file `NESSA_GITHUB_APP` names (checks point it at a fake GitHub on this machine) |
| Signed out, waiting for a code, or signed in | `GithubConnections` | memory, and the token file when signed in |
| The token and whose it is | `GithubTokenFile` | `<namespace>/github/sign-in.json`, a private file in a private directory, replaced whole and synced |
| Each step, for the owner to look back on | `DurableGithubAudit` | `<namespace>/conversations/audit/github`, one private file per record, never a token or a code; a `connect-ended` record names what decided it (`github_answered`, `deadline_passed`, `github_unreachable`, `store_failed`) |
| What an agent can read through Nessa's sign-in | `GithubConnections` and the built-in `nessa-github` tool | a gateway-side request using the current sign-in, never an agent environment variable |
| What the person sees | `src/desktop/settings/ui/github-row.tsx` | nothing: it reads `github.status` |

## The flow

```mermaid
sequenceDiagram
    participant P as Person
    participant S as Settings (desktop)
    participant G as Gateway (GithubConnections)
    participant H as GitHub
    participant A as Agent opened later
    P->>S: Connect…
    S->>G: github.connect
    G->>G: audit connect-requested
    G->>H: POST /login/device/code (client_id)
    H-->>G: device_code, user_code, verification_uri, interval
    G-->>S: connecting {code}
    S-->>P: the code, and Open GitHub
    loop every interval, until approved, refused or expired
        G->>H: POST /login/oauth/access_token (device_code)
        H-->>G: authorization_pending
        S->>G: github.status (every 2 s while a code is shown)
    end
    P->>H: enters the code, authorizes Nessa
    G->>H: POST /login/oauth/access_token
    H-->>G: access_token
    G->>H: GET /user (Bearer token)
    H-->>G: id, login
    G->>G: audit connected, save sign-in.json, then Connected
    S->>G: github.status
    G-->>S: connected {account}
    A->>G: read_repository(owner, repository)
    G->>G: verify source turn and current sign-in
    G->>H: GET /repos/owner/repository with Bearer token
    H-->>G: bounded repository answer
    G-->>A: repository answer, without token
```

The person then picks which repositories the app may reach from **Choose
repositories** (the app's `installations/new` page). A user-to-server token
can only reach repositories the app is installed on *and* the person can
reach, so that choice is GitHub's, not Nessa's.

## What an agent gets

The Nessa GitHub sign-in remains in the gateway. Neither provider opens nor
the managed shell receive its token or a git credential helper. The built-in
`nessa-github` tools admit a live source turn through Cedar, bind the operation
to GitHub's API host and a validated repository identity, and read the current
sign-in at the call. The first operations are `read_repository`, `read_issue`,
and `read_pull_request`; their arguments select only the owner, repository, and
an optional issue or pull request number. An API redirect is never followed,
and the returned body is bounded. The gateway returns the answer, never the
authorization header.

The Nessa sign-in no longer authenticates raw `gh`, `git clone`, or `git push`
commands. Those commands can still use a person's separately configured
machine credentials, which Nessa does not inspect or supply. Repository writes
need a broker with durable retry and effect audit semantics; the read tools do
not imply that a write was authorized or completed.

| Prior state and event | Effect | Later order or retry |
| --- | --- | --- |
| No connected sign-in; agent requests a read | Refuse before network access | A later sign-in may enable a new call |
| Agent source or operation arguments are invalid | Refuse before token access or network | Corrected arguments require a new call |
| Connected; Cedar refuses the source turn | No token access or network | A later owner-initiated turn is checked anew |
| Connected; Cedar allows a read | Gateway uses the current token for one fixed-host, repository-scoped GET and returns bounded data | Repeating the GET reads current GitHub state |
| Read in flight, then Disconnect | Disconnect waits for the bounded read to settle before deleting the token | No read that starts after Disconnect returns can use that sign-in |
| Disconnect completes, then read | Refuse before network access | A newly connected sign-in is a different authorization |
| GitHub refuses, redirects, times out, or returns an oversized answer | Return a typed failure without the token | No local GitHub state is inferred from the failure |
| Agent or shell process opens before or after Connect or Disconnect | No Nessa token or helper is placed in its environment | Process restart cannot resurrect an earlier Nessa sign-in |

## Orderings

Every change takes `changes` first (a tokio mutex), so each sees the state
the last one left. A read (`status`, `token`) takes only the state and never
waits on GitHub. The poll is a task of its own holding the owner weakly; what
it finds applies only while its own attempt (`operation_id`) is still the one
waiting.

| # | State | Event | Result | Record |
| --- | --- | --- | --- | --- |
| G1 | signed out | connect | `connect-requested` recorded first; a record that cannot be kept refuses `github_audit_unavailable` and nothing changes | connect-requested |
| G2 | signed out | connect, GitHub unreachable, rate-limited or answer unreadable | signed out, `last: unreachable`, refused `github_unreachable`; an end that cannot be recorded adds `unrecorded` | connect-ended |
| G3 | signed out | connect, GitHub refuses the app | signed out, `last: rejected`, refused `github_rejected`; an end that cannot be recorded adds `unrecorded` | connect-ended |
| G4 | signed out | connect, code given | connecting; poll starts at GitHub's interval (never under 1 s) | — |
| G5 | connecting or connected | connect | answers the state as it is: asking again is safe | — |
| G6 | connecting | poll: pending, a lost answer, or a rate limit (403, 429) | keeps waiting | — |
| G7 | connecting | poll: slow_down | keeps waiting at the larger interval | — |
| G8 | connecting | poll: expired, or the code's time passes | signed out, `last: expired` | connect-ended |
| G9 | connecting | poll: denied | signed out, `last: denied` | connect-ended |
| G9a | connecting | any end (G8, G9, G11, G12, G14) whose record cannot be kept | ends all the same, the cause kept in `last`, with `unrecorded` | — |
| G10 | connecting | poll: token; account read | `connected` recorded, then the file saved, then connected | connected |
| G11 | connecting | token; account read refused | signed out, `last: rejected`; the token is dropped | connect-ended |
| G12 | connecting | token; account read never answers (or keeps failing) before the code would expire | signed out, `last: unreachable` | connect-ended |
| G13 | connecting | token; the record cannot be kept | signed out, `last: audit_unavailable`; never saved, never given out | — |
| G14 | connecting | token; the file cannot be saved | the file is deleted (a save can fail after writing it), then signed out, `last: store_unavailable`; never given out | connected, then connect-ended: the later record is what holds |
| G15 | connecting | cancel, or disconnect | signed out, `last: cancelled`, poll stopped; stands even when its record cannot be kept, which is refused `github_audit_unavailable` and adds `unrecorded` | connect-cancelled |
| G16 | signed out (after G15) | the cancelled attempt's late answer | dropped: not the attempt waiting (`a_cancelled_attempts_late_approval_is_dropped`) | — |
| G17 | connected | disconnect | `disconnect-requested` recorded first, then the file deleted, then signed out; if `disconnected` is refused, still signed out with `unrecorded`, refused `github_audit_unavailable` | disconnect-requested, disconnected |
| G18 | connected | disconnect, the file cannot be deleted | still connected, refused `github_storage_unavailable`; if `disconnect-failed` is refused too, connected with `unrecorded` | disconnect-failed |
| G19 | signed out | cancel, or disconnect | answers signed out; disconnect first deletes any token file left by G14 (refused `github_storage_unavailable` if it cannot) | — |
| G20 | (start) | the gateway starts | the saved sign-in, or signed out; a file that cannot be read is signed out with `last: store_unavailable` | — |
| G21 | connecting | the gateway stops | the attempt is gone; next start is signed out | — |

Disconnect forgets the token on this gateway. Revoking it at GitHub needs the
app's client secret, which a desktop app cannot keep, so the row says to use
GitHub's Settings › Applications for that.

## Methods

`github.status`, `github.connect`, `github.cancel`, `github.disconnect`
(`protocol/product/manifest.json`), each `credential.manage` in Cedar and
taking `{}`. Each answers a `GithubConnection` (`protocol/product/v1.json`).
`github_not_configured` when the gateway runs without agents or could not
read its app. See the [protocol table](../../protocol/README.md).

## Checks

- Owner orderings: `crates/nessa-server/tests/github/mod.rs`, with a scripted
  GitHub, a memory store and a recording audit on a manual clock.
- The HTTP answers, fixed repository path, app file, token file and audit:
  tests beside each in `github/infrastructure` and `github/domain`.
- The broker's fixed tool schemas and argument rejection:
  `composition/github_tools.rs`; the shell command environment drops the
  Nessa sign-in (`crates/nessa-mcp/src/main.rs`).
- The row: `src/desktop/settings/ui/github-row.test.tsx`.
- End to end: the stress check's `github` step (`pnpm test:e2e:stress`) runs
  a fake GitHub on this machine, connects in Settings, has a new agent report
  that no Nessa GitHub token or helper reaches its own environment or Nessa's
  shell, and reads a repository through `nessa-github`. It then disconnects
  and checks a new agent still has neither, the file is gone, the gateway log
  never held the token, and the audit holds each sign-in step.
