/** A bounded real Transcript; only this fixture owns the source controls. */
import { configureStore } from "@reduxjs/toolkit"
import { createRef } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { ClockProvider } from "../../../../src/clock"
import { DesktopIconFamilyProvider } from "../../../../src/desktop/ui/icons"
import {
  workspaceActions,
  workspaceReducer,
} from "../../../../src/desktop/workspace/adapters/store/slice"
import { initialWorkspace } from "../../../../src/desktop/workspace/application/workspace-state"
import {
  emptyTranscript,
  type Message,
} from "../../../../src/desktop/workspace/model/transcript"
import { Transcript } from "../../../../src/desktop/workspace/ui/transcript/transcript"
import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "../../../../src/desktop/styles.css"

const id = "reading"
const markdown = `## A readable reply

First paragraph.

Second paragraph with [a link](https://example.com/plan).

- Read the plan
- Check the result

1. Start
2. Finish

\`\`\`sh
echo hello
\`\`\`

<a href="javascript:alert(1)" target="_blank">raw</a>
[x](javascript:alert(1))`
const history: readonly Message[] = [
  {
    id: "markdown",
    role: "agent",
    at: 1,
    parts: [
      { kind: "text", text: markdown },
      { kind: "step", step: "read", label: "Read", detail: "plan.md" },
      { kind: "step", step: "edit", label: "Edited", detail: "result.md", added: 2 },
    ],
  },
  ...Array.from({ length: 35 }, (_, index): Message => ({
    id: `history-${index}`,
    role: "agent",
    at: index + 2,
    parts: [
      {
        kind: "text",
        text: `Saved paragraph ${index}. A long conversation stays scrollable while the reader looks back.`,
      },
    ],
  })),
]
const session = {
  id,
  channelId: "fixture",
  title: "Transcript reading",
  preview: "A readable reply",
  model: { provider: "anthropic", modelId: "claude-opus-5" },
  status: "idle" as const,
  startedAt: 1,
  updatedAt: 1,
  pinned: false,
  unread: false,
  revision: 1,
}
const store = configureStore({
  reducer: { workspace: workspaceReducer },
  preloadedState: {
    workspace: {
      ...initialWorkspace,
      status: "ready" as const,
      sessions: { [id]: session },
      transcripts: { [id]: { ...emptyTranscript(id), revision: 1, messages: history } },
    },
  },
})
let revision = 1
function publish(text: string, working: boolean) {
  store.dispatch(
    workspaceActions.transcriptLoaded({
      transcript: {
        ...emptyTranscript(id),
        revision: ++revision,
        messages: [
          ...history,
          {
            id: "live",
            role: "agent",
            at: 100,
            ...(working ? { working: true as const } : {}),
            parts: [{ kind: "text", text }],
          },
        ],
      },
    }),
  )
}
const root = document.getElementById("root")
if (!root) throw new Error("Transcript reading root missing")
createRoot(root).render(
  <Provider store={store}>
    <ClockProvider now={() => 1000}>
      <DesktopIconFamilyProvider>
        <main data-transcript-reading data-surface="desktop" style={{ padding: 24 }}>
          <h1>Transcript reading fixture</h1>
          <nav aria-label="Fixture source controls">
            <button onClick={() => publish("A streamed reply ", true)}>
              Start reply
            </button>
            <button
              onClick={() => publish("A streamed reply keeps its final words.", false)}
            >
              Finish reply
            </button>
          </nav>
          <section
            aria-label="Conversation"
            style={{ display: "flex", height: 560, width: 720, maxWidth: "100%" }}
          >
            <Transcript
              sessionId={id}
              scrollRef={createRef()}
              onHeadingVisible={() => {}}
            />
          </section>
        </main>
      </DesktopIconFamilyProvider>
    </ClockProvider>
  </Provider>,
)
