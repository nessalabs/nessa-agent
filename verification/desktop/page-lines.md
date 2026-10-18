# Page console and request lines

One owner, `verification/desktop/scripts/lib/page-lines.mjs`, reports every
console error and failed request a desktop check's page records. `openPage`
keeps its queues out of the returned page API; scripts receive classification
actions and `heldErrorCount`, not the arrays. The owner takes the collector
directly when it attaches, so a caller cannot retain a queue as result evidence.
The queue-free API and once-only reporting are tested in `page-lines.test.mjs`.
Its consumer check uses TypeScript inference to recognize the `openPage`
contract through differently named handles and wrappers, rejecting queue
member access and destructuring. It does not claim to resolve `any` or
`unknown`; the public API test separately rejects exposing the queues.
Browser regression evidence accompanies the #912 fix with its exact source.

`run.mjs` binds the check's reporter once. `openPage` watches when a
reporter is bound. `report().add` takes the lines of the page opened in the
async work that produced the result. Two pages open at once stay apart: a
worker's result does not take another worker's lines. Close takes whatever
that page still holds and reports it as `console`. `openPage` supplies the
actual browser engine and selected layout even when the caller omits explicit
line metadata, so a late console failure also fails that engine's summary cell
(#693).

| what is ready | what happens |
| --- | --- |
| a step's result is added, and its page has lines | those lines move onto that result; errors append to `failures`, harmless lines are `harmless` |
| a line arrives after that result | the next result of that same work takes it, or close does when no result follows |
| two pages are open, and each has a result | each result takes only the page its own work opened |
| close, and lines remain | await the bounded evidence/size barrier, then drain one `console` result and close the page; a context-close failure still retains that reported result |
| the result could not run, including a later step "not run" | it takes nothing; the lines stay for a real result or for close |
| open throws after the page exists | close reports the lines, the open result is "could not run", and each step not started is "not run" |
| a script inspects held errors | `heldErrorCount` publishes the count without exposing the queue; reporting still belongs to this owner |
| a script already knows a line is noise | `noteHarmless(pattern)` moves matches, including ones still to come, onto `harmless` |
| a step learns afterwards which held lines are noise | `reclassifyHeld` moves those lines, rewritten, and leaves every other error |
| the apps step proves its mount is live, with a same-origin `/mcp-resources` `net::ERR_ABORTED` held (#647) | `browser.mjs` publishes the exact endpoint pattern; `noteHarmless` moves the held match |
| that same abort arrives after the live proof, before the next result or close (#647) | the installed pattern classifies it as harmless; the existing result/close owner reports it once |
| no live proof, or an abort with another origin, path or error (#647) | no exemption applies; the existing result/close owner reports the failure |
| the dev gateway app check observes resource factories (#996) | retain exact endpoint Requests outside generic string queues until the final browser cutoff acknowledgement; step drains and harmless patterns cannot take them |
| that exact observed Request has consumed EOF, matching length/SHA256, original client acceptance and its same uncanceled live bridge | `browser.mjs` classifies only its HTTP200 `net::ERR_ABORTED` as harmless, including cross-origin; close reports it once |
| known proof is canceled, corrupt, truncated, ambiguous, missing or timed out | final direct reporting keeps the failure even with a same-origin size hint or live pattern; no older result needs revocation |
| an admitted resource Request has no terminal event after observer ACK (#996) | the installer waits for its actual requestfinished/requestfailed within the existing shared settlement budget; an unresolved candidate invalidates positive evidence at final reporting |
| that Request fails in a later event-loop task (#996 R2) | its terminal and failed-line listeners run synchronously before settlement resumes; the line owner then checks for remaining pending candidates, stops collection, classifies retained Requests and drains without an intervening await |

The opt-in observer contract and bounds are in
[resource-read evidence](resource-read-evidence.md). Its finalizer runs even
without a reporter and on opening failure; ordinary lines arriving while the
barrier is pending are taken afterward. The measurement cutoff precedes context
disposal. Other checks retain their existing origin rules.

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
