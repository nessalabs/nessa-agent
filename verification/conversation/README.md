# Committed conversation controls

`fixture.html` mounts the production `ConversationControls` and `applyView` with
current published gateway views. Every case passes the actual client decoder
before `applyView`; its review has a correlated running message and offered mode. Server projection tests own exact live execution
and command authority. This fixture checks the browser's presentation of their
published result, including re-enabling controls after an incomplete view.

Run `node verification/desktop/scripts/committed-transcript.mjs`. It shares the
desktop verification browser, result and selector owners; Chromium and WebKit
measure notice counts, enabled control counts and horizontal overflow. The fixture
is served by the development server and is not included in product assets.
The standard `run-all.mjs` default development mode runs it through the shared
server URL. `run-all --mode prod` leaves this check out and names it
"not run: dev server only"; that is not "could not run". Explicit fixture-serving
URLs are supported; a product-only preview without the fixture, run on its own,
reports could-not-run.

The production question and notification components also render one retained ask
with three real sibling fields while the interaction display-limit notice appears
and disappears. Browser checks preserve the middle field's selection, whole
question units, Stop guidance and bounded horizontal layout.

The history-tab fixture also drives the real Redux refresh, saved-tab selection
and close thunk after decoding each published history state. The browser measures
saved-reference counts and external close-port invocations, with complete-empty
as the permitted counterpart. It proves the UI consumer's dispatch decision;
it does not run a gateway or establish physical provider cleanup.
