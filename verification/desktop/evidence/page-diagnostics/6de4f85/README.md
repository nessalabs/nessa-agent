# Page diagnostic ownership evidence (#912)

These are verification-only records for the public page API boundary. Product source is unchanged.

- The six actual-browser witness records retain their exact base/head and dirty diff in `nessa-912-witness-summary.json`. Chromium and WebKit baseline clock/lease errors are reported twice; the fixed owner reports once. Identical separate events remain separate, and caller reintroduction restores duplication.
- Those runtime witnesses use base6c23dfa0f or fixed102670ddf, not a relabeled6de run. Browser/page-lines/clock/lease runtime files are unchanged between102670 and6de.
- R2 negative static controls ran on102670 plus their captured diff: the inferred-contract check rejects all eight reintroduced aliases, and destructuring/bracket reads. All probes were restored before the final source freeze.
- The R2 helper source passes193/193 plus format and ESLint. Fresh full R2 review at6de is clean, with no independent runtime executions by that reviewer.
- Full32 functional verification on clean6de and the wrapped non-admin gateway case are still pending publication-time acceptance gates. This artifact does not claim they passed or make a performance claim.

Existing close metadata behavior is unchanged: engine/layout are supplied automatically; width appears only when provided through o.lines. The static check excludes any/unknown and does not claim arbitrary dynamic-program analysis.

## Final acceptance at clean 6de4f85

The earlier publication-time pending statement above is historical. Final source-qualified records now complete those gates:

- Full functional run: 32/32 programs, 1298/1298 results, four declared message-sync retained-app dev-only skips. No performance gates were run.
- Wrapped non-admin gateway 5/5, provider sign-in 21/21, bounded workspace identity 3/3. The bounded case uses 100 sessions and two messages; it is not a scale benchmark or general-journey assertion.
- Actual Chromium/WebKit errors at final source plus captured injection: four events produce four failures; two identical events per case produce eight failures, preserving multiplicity. Both injected runs correctly exit 1. Restored clock passes 5/5.
- All probes/output artifacts restored; helper 193/193, ESLint, formatting and diff checks pass on the clean frozen head. Fresh full R2 is clean; all ten hosted checks pass, and current-head Codex found no major issues.

`nessa-912-final-acceptance.json` records sources, exact controls, artifact hashes and exclusions. Baseline and earlier fixed witnesses above keep their original source attribution. The complete JSON/logs are retained rather than relabeling or erasing earlier incomplete results. The diagnostic probe diff also records seven generated Settings screenshots changed by the full runner; these outputs were not runtime inputs and were restored.

