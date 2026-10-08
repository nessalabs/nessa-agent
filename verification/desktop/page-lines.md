# Page console and request lines

One owner, `verification/desktop/scripts/lib/page-lines.mjs`, reports every
console error and failed request a desktop check's page records. A script
does not splice those arrays and does not copy them onto a result.

`run.mjs` binds the check's reporter once. `openPage` watches when a
reporter is bound. `report().add` takes the lines of the page opened in the
async work that produced the result. Two pages open at once stay apart: a
worker's result does not take another worker's lines. Close takes whatever
that page still holds and reports it as `console`.

| what is ready | what happens |
| --- | --- |
| a step's result is added, and its page has lines | those lines move onto that result; errors append to `failures`, harmless lines are `harmless` |
| a line arrives after that result | the next result of that same work takes it, or close does when no result follows |
| two pages are open, and each has a result | each result takes only the page its own work opened |
| close, and lines remain | one `console` result, then the page closes |
| the result could not run, including a later step "not run" | it takes nothing; the lines stay for a real result or for close |
| open throws after the page exists | close reports the lines, the open result is "could not run", and each step not started is "not run" |
| a script already knows a line is noise | `noteHarmless(pattern)` moves matches, including ones still to come, onto `harmless` |
| a step learns afterwards which held lines are noise | `reclassifyHeld` moves those lines, rewritten, and leaves every other error |
| the apps step proves its mount is live, with a same-origin `/mcp-resources` `net::ERR_ABORTED` held (#647) | `browser.mjs` publishes the exact endpoint pattern; `noteHarmless` moves the held match |
| that same abort arrives after the live proof, before the next result or close (#647) | the installed pattern classifies it as harmless; the existing result/close owner reports it once |
| no live proof, or an abort with another origin, path or error (#647) | no exemption applies; the existing result/close owner reports the failure |

For #647, the #473 live-mount exemption now matches the exact
`/mcp-resources` endpoint rather than any path ending in that name. Query
and fragment suffixes remain eligible. `browser.mjs` owns the shared pattern
for held and future lines; `page-lines.mjs` owns classification and reporting.

A line is removed from the page when it is reported, so a second add or a
later close cannot report it again. The close path's own result does not
take lines off a page underneath it.

`load-fallback.mjs` does not use this owner. It opens its own page and
holds the frontend's scripts back; those aborted loads are not lines, and
the check records what remains itself.
