# Actual synthetic-CI integration impact review — issue 896

Technical publication excerpt; original report SHA-256: `c35122ebfc1fb5fa4444ec74be788dcd4f5907d27ce8c6098edc28f8bbf73a46`.

This is bounded source-qualification review of actual CI inputs, not a fourth whole-feature review, restack, or reset of rounds 1–3 or their bounded closures. No implementation, source, target, cache, branch, publication, listener, browser or CI changes were performed.

## Exact scope and result

Checkout `/workspace/nessa-webhook-ci-f9e` was clean before and after inspection at `f9e906be369d7abddbad72893143283e4d95c798`. Its first parent is upstream main `be8f53f80b7cd1fb7d49de508351e8c068238483`; second parent is published PR source `c2ac8c4fff3128a182d3d9e06f99218de36c63e3`. Their merge base is `3a22008c606d6867d02f4bd242ff27944726aa3f`, also be8's parent. The published-to-synthetic difference is exactly 45 files, 923 insertions and 114 deletions, corresponding to upstream #1124 (Bonjour and owner-phone access).

No concrete outbound-webhook integration regression was identified in the inspected boundaries. This does not qualify pending CI/platform/browser acceptance. There is no source-backed reason from this review alone to restack or modify source merely because main advanced.

## Findings and disposition

- Blocker: none identified in this bounded source review.
- Major: none identified.
- Minor: none identified.
- Nit: none identified.
- Qualification remains pending: CI run `40262852364` tests the synthetic f9e tree, including changed shared Auth, native Bonjour and default linking stress inputs. Earlier c2ac or historical checks cannot establish acceptance for that assembled system. Windows/browser outcomes were pending in the parent brief; this reviewer did not fetch/reinterpret their live outcomes.

## Relationships inspected

### Authorization, principals and resources

Inspected the Auth pairing enrollment/projection/private-state changes, OwnerPhonePurpose admission, ConsentClass grants, and unchanged server owner-phone policy/socket/reset paths. Manual owner-approved PAKE enrollment now supports OwnerPhoneReadWrite in addition to signed originals. `signed_original` distinguishes the two proof paths; projection chooses its constructor from actual stored original evidence rather than consent class. OwnerPhonePurpose now accepts `owner_approved_key()`, but still requires Active, OwnerPhoneReadWrite, actual receiver binding, exact delegated credential, owner principal, membership, audience and organization. It does not infer owner authority from a coarse grant or UI fullAccess flag alone.

OwnerPhoneReadWrite grants remain conversation.read plus conversation.write and server.read; they do not add credential.manage. Existing purpose Cedar policy still default-denies unlisted product methods and does not permit webhook owner management. Webhook purpose, origin/issuance restrictions, canonical session policy, reset/cursor owner checks, target registration checks and conversation/share authorization remain unchanged. The unchanged reset handler freshly holds/admit-checks Auth, asks credential.manage (and conversation.read for requested canonical Head/reset), then verifies actual target/registration and conversation ownership. This change does not grant paired/webhook credentials reset, issuance or share management authority.

The pairing enrollment diff changes proof/class predicates without reordering mutation_registry/registry acquisition or commit operations. The canonical registry mutation fence, session/origin/revocation modules and Cedar adapters are identical Git objects. No new shared lock acquisition or hold through webhook source/transport I/O was introduced by these incoming changes. This is integration analysis, not independent proof of every new phone enrollment state.

### Root wiring and physical ownership

Root wiring changes only the native linking announcer factory. On non-macOS it retains MulticastAnnouncer; on macOS SystemAnnouncer owns a DNS-SD reference, withdraws through the same Announced boundary and deallocates in Drop. Inspected the factory, root injection and registration lifetime. Webhook runtime mounting, pin/ack/settlement, canonical reader, signed sender and shutdown wiring are identical. No webhook path starts Bonjour. This did not execute native discovery or establish macOS DNS-SD correctness.

### Protocol and client

The only changed product schema definition is PairingEnrollee, adding ownerPhone; the generated Rust/TypeScript pairing representation and client export reflect that addition. Manifest, webhook DTO/client methods, owner reset/cursor and canonical records methods are unchanged. No webhook protocol, grant or resource expansion was introduced by merge resolution.

### Native fixture, focus and browser contracts

Incoming native tests explicitly add a four-grant phone_owner constructor; existing default OWNER_GRANTS remain credential.manage and conversation.read, and with_read(false) remains management-only. The test fixture now supplies real owner-phone receivers. The PR's CedarPolicyEvaluator injection into AuthenticateSession is preserved: compared to incoming be8, this remains the only support.rs overlay. It is not replaced by a permissive session fixture.

The existing held-read cancel/focus test is byte-identical to c2ac: joint dialog-absent and parked-row focus is observed with vi.waitFor before the held read is released, then Pair focus is asserted. Incoming fullAccess additions do not loosen these assertions. UI sharing filters classify full-access phones for presentation; canonical server purpose/Cedar remains authoritative. No live browser was run, so actual UI/platform compatibility remains an acceptance gap.

### Stress and workflow

The entire async webhook step is byte-identical, SHA-256 `e43efe5a3afcc3597cac0e2d94223ca3f2b2ba952801ddefad82dd8b748cf8e5`. Shared verification helper libraries and workflow trees are identical to c2ac. The only conversation-stress change is pairFromWindow documentation and an additional exact owner-phone pending-class assertion. Existing modal/code/cancel/focus checks remain. That path belongs to linking; focused webhook cases zero linking and retain native.enabled=false. The focused webhook step still uses its actual panel owner for issuance/share/cursor/unshare, fixed observed Head and independent zero-attempt proof; it remains no-send coverage, not positive TLS evidence.

Compared with incoming be8, local-auth.yml preserves the complete 11-line focused webhook default/raised step (seed 1, webhook counts 1 and 3, existing budgets/engines) and the three-line RUST_TEST_NOCAPTURE diagnostic overlay. Complete default stress remains present before focused cases. Strict conversation helper libraries are unchanged, preserving one exact-identity click, existing 15-second budgets and joint unique visible nonzero composer observation; no replay/retry was added. Actual default stress now also exercises upstream phone-class/Bonjour behavior and needs its real CI outcome independently of focused webhook success.

## Commands and evidence

Read AGENTS.md and applicable CODING_STANDARDS review brief, all-priority closure and bounded-round rules. Used only read-only git log/status/diff/rev-parse/show, rg, sed and in-memory Python comparisons. `git diff --stat c2ac..f9e` established 45 paths/923+/114-. Python assertions passed for 22 identical Git objects/trees:

- webhook production and test trees;
- workflows and verification helper libraries;
- Auth Cedar adapters, registry owner, session, origin and revocation modules;
- server reset/socket/owner-phone handlers and local_auth composition;
- conversation application, access_policy, owner_phone_policy, record_read and share_grants;
- product manifest, conversation-api client, Cargo.toml and Cargo.lock.

Additional Python assertions passed for complete webhook stress block equality, identical held-read focus test and schema changed-def set exactly `[PairingEnrollee]`. Direct be8-to-f9e diffs confirmed native policy injection, focus-test overlay and workflow coverage/diagnostic lines preserved. An initial rg referenced nonexistent `.github/workflows/ci.yml` (exit 2); corrected inspection used actual `local-auth.yml`. This was a read-only lookup error, not a failing product check.

No build, test, formatter, browser, mutation probe or network operation was run. No passed aggregate from previous heads was represented as newly run against f9e. Final worktree status was clean.

## Explicit exclusions and next qualification

This did not review all upstream phone/Bonjour functionality, independently assess its platform FFI/test adequacy, rerun whole-feature review, execute source interleavings, or qualify physical macOS discovery/browser behavior. Direct object equality preserves prior source evidence for unchanged relationships but does not validate changed upstream shared inputs. Collect actual terminal synthetic-head CI/platform/default and focused browser results, retain original failures, and diagnose any concrete changed-input failure before acceptance. Existing denied/held operations and merge boundary remain untouched. No finding here authorizes bypass, rerun, download, permission change, public listener, live credential or live recipient.

## Late author-reported CI checkpoint

After the source report was prepared, the author reported terminal frontend success on actual f9e: 5307/5307 across 409 files including focus 15/15, build 17.44s; gateway 1037 client tests and release green. Reviewer did not fetch these outcomes independently. Platform tests/default stress remain ongoing in that update. An active Windows log lookup returned BlobNotFound404 to parent and was not retried; it is not evidence of a test failure or pass. These receipts narrow the pending frontend gap without qualifying remaining platform/default stress.
