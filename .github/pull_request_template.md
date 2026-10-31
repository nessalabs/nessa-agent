**Before:** <!-- one sentence: what a person using the software sees today -->

**After:** <!-- one sentence: what they see once this lands -->

<!-- "Closes #N" when this finishes the issue, "Part of #N" when it does not. -->

## Background

<!--
What someone new to this code needs to follow the rest, and nothing more.
Plain words. Name the parts involved and what each one is for.
One small diagram of those parts and how they talk, if it helps.
-->

## The problem

<!--
What goes wrong, in plain words, from the side of the person using it:
"you press Send twice and the message appears twice".
Then the flow near the problem, with a diagram, marking where it breaks.
Draw the parts a reader knows (the person, the app, the server), not
functions, and make each arrow a plain sentence.
Use a technical word only when nothing simpler says it.
Full guide: the pull-requests skill, "Write it from the reader's side".
-->

## The solution

<!--
What changes, in the same plain words, and the same diagram redrawn for the
new flow, over the same parts. Then the steps, each naming the commit that
does it. A list of files and what each now does belongs in the commits.
-->

## How we checked it

<!--
Short bullets in plain English: what was asked, what we did to show it now
works, and what we did not run and why. No test-name lists.
Screenshots for anything a person sees; a diagram where it helps.
For a change to UI, gateway, ACP, or MCP behavior, paste the summary from
`pnpm test:e2e:scripted` and link its evidence directory; what it contains is
in CODING_STANDARDS.md, "Browser verification for UI".
-->

## Tradeoffs and follow-ups

<!--
What this gives up (speed, memory, room to extend, simplicity), what it could
break later, the obvious approach we did not take and why, and what is left
for later with its issue link. Write "None" only if there is truly nothing.
-->

## Rollout

<!--
Breaking changes, data migrations, settings to change, and how to undo this
if it goes wrong. Write "Nothing special" when a plain merge and revert are
enough.
-->

<!-- Template owned by nessalabs/skills: skills/engineering/pull-requests/references/pull_request_template.md. Change it there first. -->
