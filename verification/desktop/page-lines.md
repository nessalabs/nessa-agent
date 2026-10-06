# Page console and request lines

One owner, `verification/desktop/scripts/lib/page-lines.mjs`, reports every
console error and failed request a desktop check's page records. A script
does not splice those arrays and does not copy them onto a result.

`run.mjs` binds the check's reporter once. `openPage` watches when a
reporter is bound. `report().add` takes the lines of the page opened most
recently. Close takes whatever that page still holds and reports it as
`console`.

| what is ready | what happens |
| --- | --- |
| a step's result is added, and the page has lines | those lines move onto that result; errors append to `failures`, harmless lines are `harmless` |
| a line arrives after that result | the next result takes it, or close does when no result follows |
| close, and lines remain | one `console` result, then the page closes |
| the result could not run, including a later step "not run" | it takes nothing; the lines stay for a real result or for close |
| open throws after the page exists | close reports the lines, the open result is "could not run", and each step not started is "not run" |
| a script already knows a line is noise | `noteHarmless(pattern)` moves matches, including ones still to come, onto `harmless`; `noteHeldHarmless()` does that for the lines held now |

A line is removed from the page when it is reported, so a second add or a
later close cannot report it again. The close path's own result does not
take lines off a page underneath it.

`load-fallback.mjs` does not use this owner. It opens its own page and
holds the frontend's scripts back; those aborted loads are not lines, and
the check records what remains itself.
