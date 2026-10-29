# Desktop matrix repair (#1203)

The repaired fixtures reach the existing UI through visible controls. Pairing
dialog closure now returns focus explicitly to its row in WebKit, where a pointer
click can leave the previous settings control focused. Pane lifecycle is unchanged. The earlier tooltip, home-shape and sideways-drag
reports are resolved by the intervening workspace change (#1197).

## Source and environment

- Base: `c92faad8e3ea1667ef7b9f5bf5d1847b32a57f19`.
- Executable verification changes: `2e8e26ee1b011ab781bdc67cac04ca7eff4bb42e`.
- Initial narrow-fixture review: `094019a712bf596eba7e1f712e0274f340f6c85b`.
- Final reviewed source: `30a58f4e25121a2f503dc3c3c1ed30ffa1a432f7`, including pairing
  focus and the additional fixture repairs found by the complete matrix.
- macOS, isolated Vite cache and loopback port; Playwright Chromium and WebKit.
  These are browser results, not native WKWebView or performance measurements.

## Reproduction and repair

| Case | Before | Restored fix |
| --- | --- | --- |
| Split motion, columns at 760×700 | Split refused because side columns leave insufficient room | Reclaim side-column room, then measure the admitted split |
| Retain reply, columns at 760×700 | Click times out on hidden composer | Reclaim room before opening and typing into the two-pane fixture |
| Retain reply/Stop, sidebar at 760×700 | Hover times out on folded session rows | Reveal the rows before using removal controls |
| Session-list requests in sidebar layout | Intentional `CannotRun` although that layout has no session list | Explicit inapplicable result, excluded from pass totals |
| Exclusion summary | Every reason is called “dev server only” | Count not run; each result retains its actual reason |

Each setup fix was individually reverted, with the changed hunk printed before
the probe. The original failure returned. Restoring the code passed the same
narrow scenarios. The summary regression likewise failed with the old reporter
and passed after restoration. Assertions about animation, closing-frame content,
removal identity and final pane count remain intact.

## Results

- Before this patch on current main: smoke **33/33**, icon buttons **13/13**,
  home-shape **3/3**, targeted drag **25/25**, focus **93/93**.
- Restored narrow matrix: **15/15** applicable checks across both browsers and
  layouts; two session-list cases are explicitly inapplicable.
- WebKit selected matrix at 1440×900, 1000×700 and 760×700: **27/27** applicable
  checks; nine session-list cases are explicitly inapplicable.
- Verifier unit suite: **538/538**. ESLint, Prettier, architecture, and documentation
  catalog validation passed. Documentation catalog: **137 pages, zero errors**.
- Independent second review: clean; independently ran **54/54** focused tests
  and four combinations of exclusions and failures.

The initial narrow repairs merged in [PR #1204](https://github.com/nessalabs/nessa-agent/pull/1204).
The additional repairs, full-matrix report, and current CI results are recorded on
[PR #1205](https://github.com/nessalabs/nessa-agent/pull/1205).

## Fixture surfaces

The narrow fixture reaches two readable panes before typing an unsent reply:

![Two readable panes at 760px](readable-panes.jpg)

It reveals the session list before hovering the removal actions:

![Session removal actions](removal-actions.jpg)

## Additional matrix repairs

The complete run exposed stale committed-review/question fixtures, missing
channel-list and tunnel responses in strict gateway mocks, and a tunnel check
that sampled its loading state. The fixtures now use the current published
contracts and wait for the state they assert. Restored committed controls pass
121/121; unread-conversation checks pass 5/5; linked-device checks pass 19/19.

The pairing focus regression fails before the runtime fix (17/18 unit tests)
and with that fix reverted in a separate checkout. The restored suite passes
18/18. WebKit now places focus on Show code after Escape closes the modal:

![Focus returns to Show code in WebKit](pairing-focus.png)

The subagent panel check now measures the existing toolbar boundary and opaque
theme background, as the production styles and their test require; its four
browser/layout cases and console check pass 5/5. Strict mock responses preserve
the unexpected-method assertion; share-dialog passes 27/27 after answering the
channel-list and tunnel reads. Linking stress setup handles a vault unlock prompt
before selecting a conversation after a prior engine restarted the gateway.

The pane and pairing state documentation was reviewed. Pairing's existing
Shown/Hidden transitions remain unchanged; its focus destination is now explicit.
The other repairs change verification data and preconditions, so they introduce
no product state transitions requiring another state-diagram update.

## Completed functional matrix

The functional matrix totals **1,631 passing applicable checks across 37 suites**.
This is a consolidated report, not one uninterrupted run: the initial five suites
completed before the runner was interrupted, the remaining 32 suites ran to
completion, and repaired suites were rerun. The original failures remain recorded
below. The subagent total replaces its four panel cases with the repaired rerun;
its other nine original cases passed. No failing result was counted as a pass.

The final selector repair is `8c7735d4c`: question actions address the conversation
pane instead of matching both that pane and Needs you. Whole-desktop and panel
removal assertions remain intact. Turn outcomes passes **13/13** in Chromium and
WebKit, both layouts. Reverting the selector produces the original strict-match
failure; restoring it passes. This verification-only change adds no product
state transition, so it does not advance a state-page source checkpoint.

| Original failing suite | Original result | Repaired result |
| --- | --- | --- |
| Committed transcript | 0/2; fixture startup prevented expansion | 121/121 |
| Conversation unread | 1/5 | 5/5 |
| Linked devices | 18/19 | 19/19 |
| Share dialog | 3/27 | 27/27 |
| Subagents | 9/13 | 13/13, four panel cases replaced |
| Turn outcomes | 6/12; opening failed before console check | 13/13 |

The 22 exclusions are 18 session-list checks in sidebar layout, which has no
session list, and four dev-only retained-app checks in the production
message-sync run. Performance was excluded with `--skip-perf`; these fixture and
focus repairs make no performance claim. Native WKWebView was not measured.

| Suite | Passed | Not run |
| --- | ---: | ---: |
| focus | 93/93 | 0 |
| shared-controls | 33/33 | 0 |
| icon-buttons | 13/13 | 0 |
| resize-observers | 5/5 | 0 |
| smoke | 33/33 | 0 |
| drag | 189/189 | 0 |
| responsive | 35/35 | 0 |
| safe-area | 91/91 | 0 |
| columns | 283/283 | 18 |
| committed-transcript | 121/121 | 0 |
| local-tab-lifetimes | 25/25 | 0 |
| lease-details | 109/109 | 0 |
| load-fallback | 16/16 | 0 |
| gateway-states | 15/15 | 0 |
| conversation-unread | 5/5 | 0 |
| mode-publication | 3/3 | 0 |
| panel-list-follow | 3/3 | 0 |
| linked-devices | 19/19 | 0 |
| share-dialog | 27/27 | 0 |
| session-rows | 6/6 | 0 |
| settings-page | 25/25 | 0 |
| widgets | 49/49 | 0 |
| subagents | 13/13 | 0 |
| mcp-apps | 101/101 | 0 |
| app-review | 37/37 | 0 |
| approval-deadline | 3/3 | 0 |
| clock-snapshots | 5/5 | 0 |
| message-sync | 15/15 | 4 |
| command-order | 9/9 | 0 |
| published-files | 25/25 | 0 |
| transcript-notes | 17/17 | 0 |
| transcript-unreadable | 5/5 | 0 |
| tool-disclosure | 21/21 | 0 |
| user-attachments | 145/145 | 0 |
| turn-outcomes | 13/13 | 0 |
| send-failure | 15/15 | 0 |
| refused-messages | 9/9 | 0 |
| **Total (37 suites)** | **1631/1631** | **22** |

## Production scripted run

Source behavior: `30a58f4e2`; subsequent changes affect verification selectors
and documentation only. Command: `pnpm test:e2e:scripted --mode prod` with an
isolated output directory. Summary:

Verdict: pass

| Check | Chromium | WebKit |
| --- | --- | --- |
| resize-observers | pass | pass |
| mcp-apps-gateway | not run: dev server only | not run: dev server only |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:61498/favicon.ico) (harmless)
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:61917/favicon.ico) (harmless)
- console chromium: console.error: Failed to load resource: the server responded with a status of 404 (Not Found) (http://127.0.0.1:62301/favicon.ico) (harmless)
