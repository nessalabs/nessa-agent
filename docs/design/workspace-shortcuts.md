# Workspace shortcut ownership — issue 819

`ui/layouts/shortcuts.ts` publishes the workspace's chords and commands.
`adapters/dom/shortcuts.ts` matches that data using the window's platform matcher
and consumes a matched key before invoking its handler. The command may be
unavailable while a modal is open; that does not transfer the bound key to a
native New Window or Close Window action. Unmatched keys retain their existing
owner. There is no extra modal keyboard state or list of forbidden shortcuts.

| Row | Case | Result | Evidence |
| --- | --- | --- | --- |
| KB1 | Matching chord, available command | Consume key, perform command | Mounted key-bindings tests; browser ordinary split |
| KB2 | Matching chord, unavailable command | Consume key, no workspace mutation | Mounted tests; browser switcher Cmd+N/Cmd+W |
| KB3 | Handler throws | Key remains consumed, original error observable | Mounted exception test |
| KB4 | Unmatched key | No command, key remains unconsumed | Mounted test and browser Cmd+X |
| KB5 | Close switcher after blocked commands | Existing ordinary commands work | Browser switcher and split sequence |

The browser check records real key events and `defaultPrevented` in Chromium
and WebKit in both layouts. It complements the existing keyboard burst: a
headless browser can survive an unconsumed native shortcut without demonstrating
that its default was prevented. The reported Fastify-style popup error remains
unattributed; this change addresses the independently reproduced shortcut
fallthrough, not an identified server handler.
