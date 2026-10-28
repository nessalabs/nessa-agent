# Fresh closing review round 2: #1158 mainline integration

**Result: no findings at any priority within this closing integration scope.**

## Exact source

Checkout `/Users/nessa/.codex/worktrees/github-webhooks-1107/nessa-agent`, HEAD
`fd4dbda66e8e6aa4cd1e27fb8d204e2f33a12505`, with the dirty in-progress merge of
main `d0d253e0201d8644a47f001df97e17f39bb92545`. I inspected working files and actual
diffs relative to both parents, including the two closing corrections, rather
than treating the prior review reports as approval of changed source.

The ordered integration/closing file set below has combined SHA-256
`28ee347d63bcd35bcc16de195bb16cf3a93bd3eea1b43ae3a3fe84deeacfd297` (path-NUL-content concatenation). Full dirty `git diff --binary HEAD`
had SHA-256 `810257f5db3f1390376edba335729a24408120474dab15e61795e9ec98d2e9ea` during this review. These identify this dirty review;
any later source change needs its own attribution.

## Closing findings verified fixed

- `composition/github.rs` now describes its normal encrypted app-session vault
  entry, not an OS sign-in entry. I traced the actual composition: it supplies
  `VaultSecret` for `github-sign-in` to `GithubTokenVault`; legacy OS import is
  separately composed through the vault migration owner. The comment now agrees
  with the current owner without claiming legacy import disappeared.
- Main's `paired_lists_keep_channel_ids_without_widening_their_scope` fixture now
  supplies `ChannelExecution::default()` and imports that value type. The actual
  `WorkspacePlacement` default is Local, and both coordinator/thread fields use
  it. This constructs the intended existing Local channel; it does not add a
  fallback to Boat allocation or weaken the real socket's grant-scoped list,
  hidden-channel wake, channel-ID and update-time assertions. The new field closes
  the coordinator-observed E0063 integration error. I did not claim compilation
  success from reading the fixed initializer; final Cargo results remain owned
  by the coordinator.

## Relationships inspected

- Read AGENTS.md and the canonical local review/closure/agreement and relevant
  gates. Inspected ADR 1152's preservation table, actual conflict resolutions,
  both protocol source owners and generated registration, CI partition checks,
  CLI transport features, placement/drop CSS and verification setup.
- The combined manifest retains tunnel, GitHub live events, vault and Boat
  methods. `dispatch_authorized` retains their distinct branches behind the
  common authorization path; the connection loop attaches its unique vault
  owner after authentication and closes it before request handles are discarded.
  Product state separately registers vault, workspaces and tunnel, rather than
  routing the latter through the vault owner or treating a tunnel as authority.
- Challenged stale credential publication by reading `GithubConnections`' captured
  generation mutation gate, `Saving` projection, `publish/in_generation`, stale
  retirement and `VaultSecret`'s `read_in/write_in/delete_in`. The vault checks
  the live owner and generation under its state mutex before storage work. A
  stale request cannot use the new session's scoped credential operations; the
  public token/status checks use current cryptographic availability. The saving
  completion retains original terminal evidence ownership and only publishes
  Connected after audit/storage receipts. This merge does not replace those
  checks with current-session unscoped storage or duplicate them in the tunnel.
- Read `GithubTokenVault`'s private bounded legacy-file read and verified-write
  migration, plus the vault composition/migration dispatch. Normal writes/read
  receipts use the encrypted scoped secret. Legacy source removal is after an
  equivalent saved value; disconnect removes the migration source first. This
  review did not add another migration or credential owner.
- Read the SQLCipher inbox composition: its key remains the separate
  `github-inbox-key` vault entry; stage configuration gates source construction
  and the GitHub App identity must match the configured relay. The mainline
  tunnel does not substitute a relay URL, introduce a production dev fallback,
  or make the inbox's key part of a webhook/socket payload.
- Additive UI resolutions retain both Boat placement and main's file-drop CSS,
  with separate balanced groups. Stress setup retains both disposable GitHub
  environment and fake cloudflared. The Linux 22.04 baseline remains checked by
  the desktop harness, while the aggregate derives the declared partitions and
  retains the vault-inclusive eight-package list. No test was weakened or
  excluded by those resolutions.

## Independent evidence

- Independently ran `node --test scripts/ci-result.test.mjs scripts/desktop/ci-harness.test.mjs scripts/ci-harness.test.mjs`: **27 passed / 0 failed**.
  Log: `/tmp/nessa-1158-final-review-ci.log`.
- `git diff origin/main --check` passes on the reviewed integration diff.
- Inspected the paired fixture diff against main and the actual Local default,
  normal encrypted owner composition, generation fences, socket connection
  ownership and authorization path, not just result summaries.

Coordinator-reported 5,436 frontend tests, 44 Worker tests and build results are
supporting evidence with coordinator attribution. Final all-target Rust checks,
actual browser/native checks and fresh hosted CI are still coordinator-owned;
this report does not relabel a running check or a broad browser baseline failure
as a pass.

## Exclusions

This is a fresh closing integration/security-regression review, not an external
pentest or a new exhaustive audit of every line in the 374-file combined feature.
No Cargo, browser, real Boat provisioning, Cloudflare deployment or source-mutating
revert probe was run by this reviewer. Prior scoped reviews remain separate
history; this report closes the stale-comment and fixture/value agreement scope
and finds no additional actionable issue in the integration seams inspected.
No source or shared cache was changed by this reviewer.

## Fingerprinted files (ordered)

- `.github/workflows/local-auth.yml`
- `crates/nessa-server/Cargo.toml`
- `crates/nessa-server/src/cli/entrypoint/arguments.rs`
- `crates/nessa-server/src/product/state.rs`
- `protocol/product/manifest.json`
- `protocol/product/v1.json`
- `scripts/ci-result.test.mjs`
- `scripts/desktop/ci-harness.test.mjs`
- `src/desktop/workspace/ui/channel-creation/new-channel.css`
- `verification/desktop/scripts/conversation-stress.mjs`
- `verification/desktop/scripts/smoke.mjs`
- `crates/nessa-server/src/composition/github.rs`
- `crates/nessa-server/tests/product/socket/subscriptions/paired_lists.rs`
- `docs/adr/todo/1152-local-vault.md`
