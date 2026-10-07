# Transcript polling follow-up (#616)

Reviewed base: `86cbce188` plus this branch's source, fixture and runner changes.
Mac17,3, arm64, macOS 26.6, 24 GiB RAM; bundled Playwright browsers, headed.
Three fresh-page runs per engine/layout; Chromium calibrated 4x CPU, WebKit unthrottled.
The summary stays idle while the retained turn runs with text after send.

| Engine / layout | Active median / max ms | Held median / max ms | Frame max / median run-max ms | Frames over 50 ms |
| --- | --- | --- | --- | --- |
| Chromium / columns | 222.8 / 518.9 | 175.1 / 508.7 | 17.7 / 17.7 | 0 |
| Chromium / sidebar | 239.0 / 511.6 | 175.5 / 456.0 | 17.7 / 17.7 | 0 |
| WebKit / columns | 172 / 406 | 101 / 221 | 20 / 20 | 0 |
| WebKit / sidebar | 165 / 277 | 172 / 279 | 21 / 20 | 0 |

Commands:

```sh
node verification/desktop/scripts/run-all.mjs --mode prod --only smoke,message-sync --channel bundled --headed --runs 3 --shots verification/desktop/evidence/message-sync-616 --out /tmp/616-production.json
node verification/desktop/scripts/message-sync.mjs --mode prod --channel bundled --engine chromium,webkit --layout columns --runs 1 --headed --out /tmp/616-browser-reverted-direct.json
node verification/desktop/scripts/run-all.mjs --mode prod --only message-sync --channel bundled --engine chromium,webkit --layout columns --runs 1 --headed --out /tmp/616-browser-restored.json
```

- Original runner: could not run, fixture not served. Fixed runner: smoke 33/33 and message-sync 15/15 held.
- Source-only revert removes the retained running/queued check, preserving the new fixture and runner: both engines fail, ten active and five held samples null per engine. Exact fixed bytes restored, then runner holds 5/5.
- Four F13 injected-clock cases fail on original and pass fixed: running/queued, idle/missing summary. Full gateway-source suite 150/150 and full frontend tests 3,743/3,743 pass. Existing F5 checks rest; lifecycle fencing/pacing tests remain intact.
- Fresh read-only review reports no findings in the combined diff and diagnostic adjustment. It did not independently run browsers.

The measured boundary is controlled ready text to DOM replacement plus two animation-frame opportunities. Native compositor paint, provider startup and production gateway payload/CPU cost are unmeasured. Historical #532 evidence and limitations remain in its original directory and report.

Scripted gateway verification: complete repeat passes all three checks in both engines. Initial run failed only Chromium MCP denial with `frame.evaluate: Frame was detached`; following steps did not run. Both original and repeat results are retained. Cause is not established; a repeat pass is not proof that the intermittent failure is fixed. Tracked in #617.
