# Vault hardening qualification

Qualified locally on an uncommitted tree based on
`ba559e7e5b4fe6db332c1a65655cc3a3b2513b5a`. This is local evidence, not a hosted
CI or deployment result. The [vault design](../../../../docs/adr/todo/1152-local-vault.md)
and [security boundary](../../../../crates/nessa-vault/README.md#security-boundary)
distinguish implemented behavior from remaining architectural work.

Linux x86_64 (Debian 13), Rust 1.99.1, Node 24.19.0. Playwright Chromium
153.0.8010.12 and WebKit 26.6. Browser runtime libraries were extracted into the
workspace from verified Debian packages, without changing host packages. The
Playwright host-library catalog check was skipped because it ignores local
library overrides; both engines actually launched and executed the checks.
Product assertions, test deadlines and configured counts were preserved.

| Check | Result |
| --- | --- |
| Vault crate, including setup compatibility, namespace isolation and change fault matrix | 47 passed |
| Client-core all-target tests and doctests | 488 + 6 passed |
| Gateway vault socket tests, normal default-thread command | 10 passed |
| Rebuilt executable bootstrap and retained CLI bridge | 1 + 2 passed |
| Native Rust source/endpoint harness | 11 passed; full Tauri/platform build not claimed |
| Vault UI/adapters/controllers and client API | 55 passed |
| Settings catalogue | 26 passed |
| Plan tests | 11 passed |
| Protocol generation/grammar checks | 65 passed |
| Grouped affected CI-package Clippy, all targets, `-D warnings` | passed |
| TypeScript app/client typechecks, affected ESLint/formatting, architecture | passed |
| Nessa Docs catalog | 141 documents, zero issues |
| Vault browser flow | Chromium 2/2; WebKit 2/2 |
| Focused conversation passphrase stress, default count 1 | 8/8, both engines |
| Focused conversation passphrase stress, raised count 3 | 12/12, both engines |
| Production scripted suite | passed in both engines |

The full unselected conversation stress plan is **blocked before steps start**:
`inputFiles` requires `NESSA_PHONE_FILES_VERIFIER` from nessa-ios. Its package
uses an Apple XCFramework; that verifier is unavailable in this Linux workspace.
The baseline driver reaches the same unchanged prerequisite failure. Neither full
default nor full raised stress is claimed as passed.

Focused runs used `--only passphrase`, with all original knob values and deadlines
retained. Results explicitly carry `scope: selected:passphrase`; background turns,
followers and final settlement ran. Selection does not modify the complete plan, counts or deadlines. Unselected
steps receive no pass results.

Commands for those focused runs:

```sh
node verification/desktop/scripts/conversation-stress.mjs --only passphrase --engine chromium,webkit --channel bundled
node verification/desktop/scripts/conversation-stress.mjs --only passphrase --passphrases 3 --engine chromium,webkit --channel bundled
```

The first browser run exposed a normal unlock prompt covering lost-answer recovery;
foreground ownership and stale-source fencing fixed it. Deliberate code probes
also rejected removal of confirmation, Locked-only result validation, no-replay
classification, owner/generation fencing, scratch erasure, caller bounds,
temporary verification, publication/audit separation and transport retirement.
The original executable failed the new bootstrap/CLI acceptance; the rebuilt one
passes. Under competing independent memory-hard fixtures, an earlier parallel run had
three one-second response timeouts, including a pre-existing test. Test-only
fixture capacity now bounds independent gateways without changing those deadlines
or within-test concurrency. The rebuilt default
socket command passes 10/10.

Fresh independent source reviews report no remaining findings in implemented
scope. Non-Linux execution, a complete Tauri build and hosted CI remain unverified.
The existing lease constant-evaluation compiler warning remains unchanged.

## Production scripted summary

Verdict: pass

| Check | Chromium | WebKit |
| --- | --- | --- |
| resize-observers | pass | pass |
| mcp-apps-gateway | not run: dev server only | not run: dev server only |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |

Relevant log lines: none.

## Screenshots

![New setup guidance](setup-chromium.png)

![WebKit confirms change and sealing](changed-webkit.png)

![Recovery remains visible after a lost answer](recovery-chromium.png)
