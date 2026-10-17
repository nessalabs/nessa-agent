# Current-main qualification and diagnostic removal

PR #962 remains a scoped cooperative lease for participating macOS hosts, not a
complete fix for the legacy-binary incident. Main at qualification is
`e650b901b30b64745cd43920b5b87e82d5442981`.

## Completed qualification before cleanup

- Exact head `e1fadd55cccb631f7a18985a9e22b90c64aa1ab9`: [required CI39054531039](https://github.com/nessalabs/nessa-agent/actions/runs/39054531039)
  completed successfully, all11jobs including both Windows selections, macOS,
  Linux/native smoke, frontend, release compilation and the aggregate.
  [Job identities and terminal results](native-e1.json).
- [Exact-head Codex receipt](https://github.com/nessalabs/nessa-agent/pull/962#issuecomment-6237847693)
  reports no major issues; no inline findings were returned.
- [Browser run39052186064](https://github.com/nessalabs/nessa-agent/actions/runs/39052186064)
  passed MCP Apps, gateway window and scripted scenarios in both Chromium and
  WebKit (6/6). [New sanitized notice](browser-118.json) identifies source
  `118cc4b48736900d3fd2c1aece554ae48b23adc3`, tree
  `b63b0913442b6489db25c4e4321b9b974e494da6`, ready state and pass verdict.
  This is prior-head browser evidence, not final cleanup-head qualification.
- Independent FULL combined integration review of118cc4b4/main e650b901 was
  clean; fresh production Unix probe13/13, strict probeClippy and architecture
  passed. All eight ownership source/test hashes still match the
  [fixture manifest](../umask-fixture/source-sha256.json). The historical158-test
  harness was lost on executor restart; its earlier results are not a fresh run.

The old Windows server-library failure did not recur in the two fresh runs.
The later Windows remaining-test failure also did not recur in39054531039.
Their original causes remain unattributed; no #777 relationship or causal fix
is claimed. The temporary118cc4b4 frontend failure was reproduced locally:
its multiline diagnostic wrapper violated the existing workflow-shape assertion.
The original assertion was preserved; restored command shape passed unchanged.
No historical denied log/artifact retrieval or alternate route was used.

## Final source cleanup

The final cleanup restores `.gitattributes` and both CI workflows byte-for-byte
to main, and deletes the three temporary diagnostic helper files. No production,
ownership test, assertion, package selection, timeout or permission is changed.
The original CI contract/harness/install selection passes27checks locally.

Final cleanup-head required CI and exact-source browser qualification remain
separate gates; their terminal receipts belong on the PR. An independently
reviewed, unmerged workflow branch `codex/943-final-browser-diagnostic` at
`d6d6b140e7685fa920e958686297748dc12cfa33` retains its sanitized publisher in the
runner temporary directory before checking out the final source. It runs the
same canonical suite against that exact source and reports provenance without
requiring product-tree reporting helpers or access to denied historical assets.
This diagnostic branch is not part of PR962 and must not be merged.

## Acceptance remaining outside the scoped source qualification

Actual coordinated legacy-to-participating transition, two genuine participating
macOS bundles with a live turn, and restart/updater overlap remain unrun. The
lease cannot constrain legacy binaries. #943 stays open. Signed installed cold
launch, assembled WKWebView, Login Items and installed restart/retry remain
#562; actual startup timing remains #109. Hosted component tests and browser
checks do not establish installed user-Mac acceptance. No live service, user
bundle, credentials, Login Items or security settings were changed here.

The scoped PR must retain these limits and pass final-head/current-main gates.
The parent owns merge sequencing; qualification is not permission to merge.
