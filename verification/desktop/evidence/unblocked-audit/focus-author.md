# #730 owned focus settlement — author handoff

Commit: 19542fd00dbe8dee6b4ddd2be10b64af317d15a9. Clean checkout after source-only commit; all commands ended.

Private branch codex/730-owned-focus-frames, base97dba21f6cc3a54e9f6df20dfa1b6c39ebd59b8c in audit-native-gateway only. This is the older assembled audit base, not the parent's new unblocked main-based cut. Only the six source/test/design/check files in the narrow commit should be carried. No blocked issue was worked; no693 per-frame fixture was carried. The original layouts.test.tsx is asserted byte-identical to base.

## Ownership and scope

The mounted useFocusFollowsPane effect now owns its queued settlement frame handles through one local afterPaint helper. Cleanup retires the effect, cancels those handles, clears the tracker and stops its current target search. Delivered cancelled callbacks cannot start nested settlement or a new search. All existing pane/widget movements remain scheduled; no coalescing or new delay/bound policy. Overview return retains its two settlement frames before the existing target-search/target-paint stages. Modal and deliberate-person-focus decisions are unchanged.

The table was written first in existing ui-workspace-load.md. Architecture's existing DOM adapter map links that lifetime. Tests stay beside the hook; browser coverage remains in focus.mjs with existing shared selectors, page/engine helpers and output owner. CHECKLIST records the new numeric contract. No module, public API, production test switch, SDK/server/native/generated contract or other UI owner was added/changed.

## Public hook and revert proof

Through real desktop stores and commands, new cases cover pane/window-widget/overview cleanup before first frame, overview cleanup between its two settlement frames, late delivered cancelled callbacks for all four orderings, replacement store/owner with old callback delivery, and mounted pane/widget/overview frame order. Existing dialog/list/person-focus cases remain covered. Original baseline with initial added cases failed9/28. Final original-production source revert failed9/29; fixed final affected suite156/156 across focus29, layouts32, overview90 and overview-quiet5.

Seven independent actual source mutations were asserted and run one at a time, then restored byte-for-byte: full owner9 failures; remove late-delivery guard4; remove cleanup handle cancellation9; bypass widget route2; bypass pane route3; bypass overview outer route2; bypass overview inner route2. /tmp/audit-focus-revert-results.json and /tmp/audit-focus-revert-*.{diff,log}. The extra replacement late-delivery assertion was refined afterward without production changes; its mounted counterpart is in final156. Every existing layout setup/open assertion is unchanged, including setup12 and exact opening2 frames. No arbitrary waits or increased thresholds.

## Real production browser results

Actual full fixed source initially held20/20 Chromium columns with mash disabled. Actual original-production source was then asserted byte-for-byte and freshly built:19/20, exit1. Only focus-retired-layout failed:

- retired owner cancelled 0 of 1 settlements
- retired callbacks queued 1 new frames

Source restored exactly; final fresh production full focus:81/81, Chromium+WebKit, columns+sidebar,1440x900 (reply cases also1000x700), default120-key burst. Each new retirement row captured1, cancelled1, newRequests0, pending0 after at most12 setup paints, current focused pane1 with activeIsComposer true. Full run wall5m11.907s. /tmp/audit-focus-browser-final.{json,log}; original source/revert artifacts /tmp/audit-focus-browser-revert*. Smoke33/33 both engines/layouts on same product source, wall32.908s: /tmp/audit-focus-smoke-final.{json,log}.

The new scenario opens two real panes, holds the browser RAF boundary, moves focus through the real keyboard, replaces the actual workspace layout via its existing preference event, waits for old workspace detachment, delivers retired callbacks, measures cancellation/new requests, settles replacement work with the existing twelve-frame bound and confirms the current composer still takes focus. No private application visibility is used.

After the full focus run, the screenshot collector alone was narrowed from a noisy full pane to the existing published composerCard selector. The unchanged actual retirement function and final collector were extracted into /tmp/audit-focus-final-capture.mjs (actual function source membership asserted) and run against a fresh production build: four retirement rows plus console5/5. /tmp/audit-focus-final-capture.{json,log}; scope of this follow-up is screenshot crop only, not a product or assertion change. Final cropped Chromium/WebKit composer PNGs81–154KiB each are in /tmp/audit-focus-evidence/. They were visually inspected and are left outside the source-only commit at parent's instruction for the final PR evidence selection.

## Final checks and environment

Final affected Vitest156/156, scoped ESLint PASS, Prettier all six text files PASS, pnpm typecheck (ui check+tsc) PASS, git diff --check PASS. Logs /tmp/audit-focus-restored-final.log, /tmp/audit-focus-handoff-{lint,format}.log, /tmp/audit-focus-typecheck-final.log. Known existing overview act warnings remain warnings, no failures.

Node26.8.1, pnpm11.9.0, Vitest3.2.7, React19.2.8/jsdom30.1.0, Playwright1.63.0; local macOS10 physical/logical CPU,24GiB. Parent Rust checks ran concurrently in another frozen checkout. No artificial CPU throttling, no performance claim. Browser runs used fresh production Vite previews, never the parent's/root's gateway or artifacts. Final full focus and smoke source remained frozen; no source edits during a production check. All owned browser/build/preview/unit/lint/type processes ended before commit; parent and unblocked_panel_list notified when browsers ended.

This private proof does not claim native WKWebView or the new unblocked assembled cut: no WebKit-only failure occurred; native app was not run. No private full run-all/perf-budget; parent will run fresh final frontend, combined run-all and quiet calibrated performance gates after narrow integration and independent review. Current evidence must not be represented as latest-main assembled proof.

## Self-review and handoff

Reviewed all three raw settlement paths, nested frame ownership, cleanup order, delivered-after-cancel behavior, store replacement, mounted frame counts and current focus authority. The single effect-local retirement owner governs all queued settlement; current target search retains its own child cancellation contract. All frames already queued by store movement are tracked, not newly introduced or coalesced. Changing the owner/cancellation/routing rules separately bites meaningful public regressions. Field/layer agreement retains pane/widget destination, movement meaning and person/modal authority; backend audit concerns are unchanged. Existing source layout/maps/links remain valid. Source SHA256 hashes: /tmp/audit-focus-source-hashes.json.

Regression introduction: the uncancelled pane settlement existed in1573ff6aa8; widget settlement introduced in00f9180740; the nested overview-return staging in c13a9ed66f retained that unowned lifetime. No GitHub mutation/push/merge was performed by this author.
