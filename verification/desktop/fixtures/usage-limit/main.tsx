/**
 * The usage-limit card on both transcripts: the desktop window's transcript
 * over a fake source, and the panel's transcript beside it, both reading one
 * view. `usage-limit.mjs` drives it.
 */
import { useState } from "react"
import { createRef } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { applyView } from "../../../../src/conversation/application/usecases/apply-view"
import { conversation as newConversation } from "../../../../src/conversation/model"
import type { Conversation } from "../../../../src/conversation/model"
import { Transcript as PanelTranscript } from "../../../../src/conversation/ui/transcript"
import { ClockProvider } from "../../../../src/clock"
import { createDesktopDependencies } from "../../../../src/desktop/dependencies"
import { makeDesktopStore } from "../../../../src/desktop/store"
import {
  followWorkspace,
  loadWorkspace,
  openSession,
} from "../../../../src/desktop/workspace"
import { transcriptFrom } from "../../../../src/desktop/workspace/adapters/gateway/gateway-views"
import {
  GATEWAY_NOW_MS,
  view,
} from "../../../../src/desktop/workspace/adapters/gateway/fake-gateway"
import { Transcript } from "../../../../src/desktop/workspace/ui/transcript/transcript"
import { fakeSource, testIndex } from "../../../../src/desktop/workspace/testing"
import "@fontsource-variable/geist"
import "../../../../src/desktop/styles.css"
import "../../../../src/styles.css"

const params = new URLSearchParams(window.location.search)
const provider = params.get("provider") === "codex" ? "codex" : "claude"
const kind = params.get("case") ?? "timed"
const preference = params.get("preference") === "off" ? false : kind !== "unknown"
const recovery =
  kind === "user-action" ? "user_action" : kind === "unknown" ? "unknown" : "timed_reset"
const resetsAtMs = Date.UTC(2026, 9, 21, 16, 0, 0)
const autoResuming = kind === "timed" && preference
const usageLimit = {
  provider,
  recovery,
  ...(recovery === "timed_reset" ? { resetsAtMs, autoResuming } : {}),
} as const
const wire = {
  ...view("b", {
    usageLimitAutoContinue: preference,
    runtime: {
      agent: provider,
      model: "unknown-catalogue-model",
      modelName: "Unknown",
      provider,
      workspace: "/tmp",
      contextWindowTokens: 200000,
      reasoning: true,
    },
    messages: [
      {
        executionId: "limited",
        userText: "Continue the release notes",
        attachments: [],
        files: [],
        status: "failed",
        usageLimit,
        parts: [],
      },
    ],
  }),
  notes: [],
  reviewsWaiting: [],
  questionsWaiting: [],
  questions: [],
  permissions: [],
}

const fixture = {
  desktopCalls: [] as { enabled: boolean; initiator: string }[],
  panelCalls: [] as boolean[],
  fail: false,
}
Object.assign(window, { __usageLimit: fixture })

const index = testIndex()
const source = fakeSource(index)
const transcript = transcriptFrom(wire, 1, () => 1000, GATEWAY_NOW_MS)
source.transcripts.set("b", transcript)
const original = source.setUsageLimitAutoContinue.bind(source)
source.setUsageLimitAutoContinue = async (enabled, initiator) => {
  fixture.desktopCalls.push({ enabled, initiator })
  if (fixture.fail) throw new Error("preference failed")
  await original(enabled, initiator)
}

const dependencies = createDesktopDependencies({ workspace: source })
const store = makeDesktopStore(dependencies)
store.dispatch(followWorkspace())
await store.dispatch(loadWorkspace())
store.dispatch(openSession({ sessionId: "b" }))

function Panel() {
  const [current, setCurrent] = useState<Conversation>(() =>
    applyView(newConversation("b"), wire),
  )
  return (
    <PanelTranscript
      conversation={current}
      ground="ink"
      animateMount={false}
      streamText={false}
      emptyState={false}
      statusLabel="Ready"
      gatewayAvailable
      onOpenPaste={() => {}}
      onUsageLimitAutoContinue={async (enabled) => {
        fixture.panelCalls.push(enabled)
        if (fixture.fail) throw new Error("preference failed")
        setCurrent((held) =>
          held.remote
            ? { ...held, remote: { ...held.remote, usageLimitAutoContinue: enabled } }
            : held,
        )
      }}
    />
  )
}

const container = document.getElementById("root")
if (!container) throw new Error("missing root")
container.style.height = "100vh"
container.style.display = "flex"
container.style.flexDirection = "column"
createRoot(container).render(
  <Provider store={store}>
    <ClockProvider now={() => GATEWAY_NOW_MS}>
      <div
        id="desktop"
        style={{ display: "flex", flex: "1 1 50%", minWidth: 0, minHeight: 0 }}
      >
        <Transcript sessionId="b" scrollRef={createRef()} onHeadingVisible={() => {}} />
      </div>
      <div
        id="panel"
        style={{
          display: "flex",
          flex: "1 1 50%",
          minWidth: 0,
          height: "100%",
          padding: "12px",
        }}
      >
        <Panel />
      </div>
    </ClockProvider>
  </Provider>,
)
