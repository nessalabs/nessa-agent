# Exact-head CI fixture correction

Published source `ac6225d7d0e3ef8be69d7cd31b2e689235324687` was exercised by
[run40249699947](https://github.com/nessalabs/nessa-agent/actions/runs/40249699947).
Its failures are preserved in the [selected original log lines](ci-ac6225-failures.txt).

Ubuntu's server library reported 4118 passed, seven failed and11 ignored. All seven
failures opened Auth directly under a default-permission temporary container. They
reproduced with the old compiled source at process-local umask022: five composition
failures and two runtime failures, both commands exit101. The correction creates
a private child with the existing local-storage helper and retains its parent
lifetime. All file-effect and refusal assertions inspect that actual child.
Production permission checks and all assertions are unchanged.

Default Chromium stress reported39/50 held and stopped at step036 webhook with
`conversation_not_found`. The panel created the conversation while a separate
bootstrap-owner principal tried to share it. The existing panel grants already
include credential management. The corrected step uses that same panel principal
for issuance, share, cursor/Head observations and unshare; authorization is unchanged.
Budgets, exact-identity selection, fixed Head and independent no-attempt proof remain.

The three-file correction is `4be5703a83f742ef4c48d6e4e2d8a589fb047d0c`.
[Affected independent review](ci-fixture-review.md) found no findings. This is an
affected CI correction review; whole-feature round-three provenance and closure
remain unchanged. No production state transition changed, so state diagrams and
their implementation source checkpoints remain valid without advancing them.

[Restored local controls](ci-fixture-local-controls.txt), all at that correction:

- Composition6/6 and complete webhooks295/295 under process-local umask022,
  including both runtime controls and all three actual retention operations.
- Required seven-package all-target Clippy passed with `-D warnings`.
- Existing stress helper controls24/24, JavaScript syntax, pinned Prettier and
  architecture checker passed. Initial architecture and SDK-documentation invocations lacked Cargo
  in PATH; both corrected declared-toolchain invocations passed.

Original CI also executed scripted Chromium/WebKit checks8/8, usage-limit checks
16/16, pane-drag checks8/8 and chaos2/2 successfully. These are source-qualified
results for ac6225, not a corrected default-stress pass. The later default steps
and WebKit menu step were not qualified after the failure. Corrected real browser
stress, raised webhook count3, webhook-specific WebKit and complete platform
qualification remain required. Helper tests are not browser or positive TLS proof.
The separate295-test suite contains the real isolated signed-recipient evidence.

## Accepted recipient socket mode

The original macOS job121338743669 then reported4101 passed,21 failed and13 ignored:
the same seven private-root failures plus14 recipient tests failing at the same
TLS read with `WouldBlock` code35. The [original Mac lines](ci-ac6225-macos-failures.txt)
retain that distinct failure evidence. The fixture's listener is nonblocking for
owned stop/join, but its synchronous accepted TLS socket had no explicit mode.
[Apple's accept contract](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/accept.2.html)
describes inherited properties;
[Linux's portability guidance](https://man7.org/linux/man-pages/man2/accept.2.html)
requires explicitly setting accepted flags instead of relying on inheritance.

Correction `c59640811a937821cb82d42d69402fa52bd23947` explicitly makes only the
accepted fixture connection blocking before the existing3-second read/write
watchdogs. The listener remains nonblocking. Real TLS bytes, signature, retry,
no-send, recovery and join assertions are unchanged. No production code or
timeout budget changed. Actual corrected macOS execution is still required;
a Linux pass does not establish it.

[Affected recipient-mode review](ci-recipient-mode-review.md) found no findings.
The [restored complete webhook suite](ci-recipient-restored.txt) again passed
**295/295** at c59640811 under process-local umask022, including all three
retention controls and real signed TLS product recipient tests. The independent
mode emulation caught WouldBlock with the old mode and read delayed bytes after
the exact correction; it is not macOS or TLS acceptance.

The required seven-package all-target Clippy selection also passed at c59640811
with `-D warnings`; no code changed after these restored tests and checks.

## Original run terminal outcome

Run40249699947 is terminal failure. Seven substantive jobs passed: classification,
workflow validation, frontend, gateway contract, SDK domain coverage, release build
and Windows non-server qualification. Ubuntu, Mac, Windows server and default
conversation stress failed as described here; the required-check aggregate failed
accordingly. This run was neither cancelled nor rerun.

Windows server job121338743616 completed3375 passed,20 failed,9 ignored in1726.53s;
it did not hit the job deadline. Its six private-root failures and14 recipients
correlate to the same repaired fixture paths. Recipient reads explicitly reported
WouldBlock10035. [Original Windows lines](ci-ac6225-windows-failures.txt) also show
all three actual retention controls passing. Both formerly failing duplicate-history
cold-restore/live-publication cancellation tests passed. The separate Windows job
passed native enrollment110/110, SDK860/860 with6 ignored, all-target Clippy,
the exact Task Scheduler process/cleanup proof and private-file smoke checks.

These are old-source outcomes. Corrected exact-head platform/browser execution
remains pending normal publication, with no manual retry or dispatch substitution.
