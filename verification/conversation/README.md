# Committed conversation controls

`lease-fixture.html?case=<state>` mounts the production details sheet over one
published lease, decoded by the actual client validator; `ssh-live`,
`ssh-lost` and `ssh-refused` are leases on an SSH host, and `peer-live`,
`peer-lost` and `peer-refused` on a paired machine.
Its view and lease cases use the SDK's published types, and this folder is
included in the normal TypeScript check; required fields and enum changes are
checked before a browser run.
`run-on-fixture.html?case=none|hosts|paired` mounts the production composer
tray with no SSH host, two, or one and a connected paired machine. Run `node verification/desktop/scripts/lease-details.mjs`
for both. These lease/composer fixtures are served by the development server only.

`fixture.html` mounts the production `ConversationControls` and `applyView` with
current published gateway views. Every case passes the actual client decoder
and the production gateway-to-panel conversion before `applyView`; its review has a correlated running message and offered mode. Server projection tests own exact live execution
and command authority. This fixture checks the browser's presentation of their
published result, including re-enabling controls after an incomplete view.
`fixture-state.ts` supplies keyed permission/question entries and their separately
loaded details. Its tests retain incomplete-view authority boundaries and reject
the former missing-key and inline-detail inputs through the actual client decoder.

Run `node verification/desktop/scripts/committed-transcript.mjs`. It shares the
desktop verification browser, result and selector owners; Chromium and WebKit
measure notice counts, enabled control counts and horizontal overflow. The fixture is served by the development server for ordinary functional checks.
`committed-transcript.mjs --mode prod` builds it with the application's Vite
configuration plus `verification/conversation/vite.config.ts`, then checks the
minified production components; it is not added to packaged application inputs.
`run-all --mode prod` starts this fixture preview separately from the product
preview. Explicit fixture-serving URLs remain supported.

The production question and notification components also render one retained ask
with three real sibling fields while the interaction display-limit notice appears
and disappears. Browser checks preserve the middle field's selection, whole
question units, Stop guidance and bounded horizontal layout.

The history-tab fixture also drives the real Redux refresh, saved-tab selection
and close thunk after decoding each published history state. The browser measures
saved-reference counts and external close-port invocations, with complete-empty
as the permitted counterpart. It proves the UI consumer's dispatch decision;
it does not run a gateway or establish physical provider cleanup.

The question-target consumer mounts the real panel component, Redux control
thunk and client serializer over two decoded views with equal execution/question
IDs. Chromium and WebKit at 360, 600 and 1440 pixels assert three fields, one
initial selection, another equal-key ask’s retained middle selection, zero
inherited selections after switching conversations, and
two exact action records: an offered choice for the second conversation and null
decline for the first. This recorder does not authenticate a server or establish
current authority. The cropped screenshot lives in
`verification/desktop/evidence/question-targets/question-targets.png`.


`lifetime-fixture.html?same` restores the same remote conversation; without the
query it restores another. Both mount the production `useConversation` follow
and controls over a gated close port. `local-tab-lifetimes.mjs` measures fresh
local identity, old follow cleanup, replacement follow, original close target,
and replacement notices at 360, 600 and 1440 pixels in both engines. Follow,
stop and close counts are the fixture's (`__localTabLifetimes.snapshot`), read
when the check runs, not attributes copied at the last render
([live owner reads](../../docs/design/live-owner-reads.md)). The
fixture is served in development and is excluded from product assets.

`user-flow-ordering.html` mounts production AgentDownloads and useComposer over
controlled offer/install/send ports for #1087. The matching desktop script
checks obsolete offers after uncertain installation and cross-tab/operation
send observations. It is frontend ordering evidence, without a gateway or
physical installation claim; design rows live in `docs/design/user-flow-ordering.md`.
