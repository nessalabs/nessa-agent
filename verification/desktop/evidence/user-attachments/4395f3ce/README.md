# Desktop user attachment references — #682

Production source: `4395f3ce8a1a8cc792957460f9d675656417a246` against main `102cff8c4383b07e1fc34c695dabefa3c6d11c72` (PR #902). This supplemental evidence commit changes no source, tests, fixtures or runtime artifacts.

Current-source metadata verification is `metadata.json`: 115/115 checks in Chromium/WebKit at widths 360/600/1440, including reload, exact image encoding/size and linked path, accessible list naming, original adjacent text, absent previews/actions and horizontal overflow within 1 pixel. Screenshots in the parent directory were inspected after the final styled fixture run.

Current-source scoped production timing is `performance.json`: 8/8 including calibration, 4× throttle, three runs per interaction/layout. Worst frame 42 ms (sidebar send-home, median39 ms), budget50 ms, zero frames over budget. No speed improvement is claimed. Root Cargo/reviewer builds and all other author browser/build work were idle during measurement.

`scripted-summary.md` is the passing sequential signed-out gateway run on this checkout's separately owned/rebuilt server and UI: MCP apps16/16, gateway window13/13, scenarios8/8 across Chromium/WebKit. The later readonly type signatures and renderer test have no runtime change to that checked tree.

Focused tests305/305, final TypeScript/lint/format and a fresh full19-file review pass at the recorded source. Full frontend tests4593/346 ran before the final readonly type change and added renderer test; final affected suites include that test. Earlier full scripts capture timeout is preserved outside this evidence directory at `/tmp/nessa-682-frontend.log`; subsequent full scripts pass360 with two existing skips. The imported ACP test tree is byte-identical to exported main; `base-acp-control.txt` is its separately exported-main positive control. Its original timing cause remains unproved.

Revert evidence: mapper removal fails nine metadata tests; tile removal fails18 browser cases; providing a conversation ID makes the no-byte-lookup test observe a read and fail. Source was restored and final focused tests/browser checks pass. The renderer-removal record predates readonly signature typing, which changes no runtime rendering.

Preserved failed scripted run: `initial-scripted-summary.md` records Chromium gateway-window's aborted desktop.html navigation. All behavior cases passed; the sequential final run passes without weakening the request assertion or claiming a product fix. A source-generation/Vite reload is only a hypothesis.

The first broad functional run was stopped after a Chromium/columns overview-count selector could-not-run and is incomplete, not green. Its exact whole shared-controls check on exported main passes9/9 (`base-selector-control.json`); failure-only DOM capture was armed but no timeout occurred. The original selector timing cause remains unproved. The final frozen-source full functional matrix is pending when this evidence is published, as are current-head CI and external review. PR #902 remains draft until those acceptance gates are resolved.
