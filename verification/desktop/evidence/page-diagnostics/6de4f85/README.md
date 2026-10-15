# Page diagnostic ownership evidence (#912)

These are verification-only records for the public page API boundary. Product source is unchanged.

- The six actual-browser witness records retain their exact base/head and dirty diff in `nessa-912-witness-summary.json`. Chromium and WebKit baseline clock/lease errors are reported twice; the fixed owner reports once. Identical separate events remain separate, and caller reintroduction restores duplication.
- Those runtime witnesses use base6c23dfa0f or fixed102670ddf, not a relabeled6de run. Browser/page-lines/clock/lease runtime files are unchanged between102670 and6de.
- R2 negative static controls ran on102670 plus their captured diff: the inferred-contract check rejects all eight reintroduced aliases, and destructuring/bracket reads. All probes were restored before the final source freeze.
- The R2 helper source passes193/193 plus format and ESLint. Fresh full R2 review at6de is clean, with no independent runtime executions by that reviewer.
- Full32 functional verification on clean6de and the wrapped non-admin gateway case are still pending publication-time acceptance gates. This artifact does not claim they passed or make a performance claim.

Existing close metadata behavior is unchanged: engine/layout are supplied automatically; width appears only when provided through o.lines. The static check excludes any/unknown and does not claim arbitrary dynamic-program analysis.
