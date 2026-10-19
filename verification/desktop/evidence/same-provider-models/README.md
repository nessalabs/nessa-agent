# Same-provider model control: current source evidence

Runtime source `76e335294` follows integration `f9b861d7` of immutable main
`c0119e8b35b8b537b701638ee87a1a682c74cff8`. UI pin is
`7a8fe71fe996305a71baba319387df6e81ffbe0e`. This separate evidence commit
refreshes only these six existing files. [Qualification](qualification.json)
records commands, hashes, scopes, causal controls and retained historical proof.
The [canonical summary](canonical-summary.md) keeps the short gateway lines.

| Current affected check | Result |
| --- | --- |
| Full affected server | 3,507 runtime passed, 13 ignored, 4 doctests; nested child summaries excluded |
| Protocol/client core | 594 runtime and 8 doctests passed |
| New SDK paged-storage case | 1 passed |
| Full frontend with new UI pin | 4,971 tests and complete frontend contract passed |
| Seven-package all-target strict Clippy, fmt and strict SDK docs | Pass |
| Rebuilt own gateway canonical three programs | Pass in Chromium and WebKit |
| Session rows / Share / Linked devices / unread | 11/11, 24/24, 17/17, 5/5 |
| Full Claude default / six model changes, both engines | 65/65 / 76/76 |
| Model-only Codex one / six changes, both engines | 9/9 / 19/19 |
| Default chaos seeds 1 and 2 | 2/2 |

Full Claude retains rename 1, delete 1, await 1, fresh Home 2, window 1 and
linking 1 defaults, with model changes last in Ask. Focused Codex sets every
nonmodel knob to zero, including rename/delete; it is model-only coverage.
Actual window assertions retain original review identity/options, unsent draft
and Stop, answer the review, and verify the selected model in the native prompt.
Claude subsequent responses and Codex next-turn effects stay distinct.

## Session-row causal evidence

The first unobserved run failed 10/11. Lightweight tracing reproduced the wrong
row opening: a press released before drag preparation became an ordinary click;
the live first-unselected locator then retargeted while its assertion retained
the original id. Anchoring alone exposed the unchanged no-open assertions (4/5).
Waiting for existing prepared/carrying DOM marks plus anchoring passed 5/5;
sole readiness omission failed 4/5; exact restoration passed 5/5. Identity omission
with readiness passed 5/5 and is not claimed as a negative control. Normal,
unobserved both-engine verification then passed 11/11. No product/vendor changes,
fixed sleeps, deadline extensions or relaxed assertions were introduced.

The stress wrapper mistakenly scheduled a second chaos invocation after the
first had already run its default two seeds successfully. That redundant partial
invocation was stopped through its owned process group and excluded. The wrapper
exit -15 is not counted green; the five intended commands completed successfully.
All owned runtime jobs retired; gateway SHA stayed exact before and after.

Earlier floor-interrupted server compiles are preserved separately and are not
test verdicts. No whole 6,838 rerun, broad SDK run, performance or installed-app
parity is claimed. Newer main after immutable c011 remains outside this scope.
Fresh whole-assembly review, current CI and prospective-source validation remain
required before merge. Native linking used only owned ephemeral configuration.

## Inspected current screenshots

- Claude/WebKit default: confirmed Fable 5.1, subsequent-responses text, draft and
  Stop; review offscreen, with its identity/options covered by assertions.
- Codex Chromium and WebKit: original review, draft, GPT-6 Astra, next-turn text
  and Stop. Both pictures come from the one-change model-only run.

![Claude: confirmed model, draft and Stop](claude-webkit.png)

![Codex Chromium: original review, model, draft and Stop](codex-chromium.png)

![Codex WebKit: original review, model, draft and Stop](codex-webkit.png)
