/** The floating panel's real transcript, including its typed error-divider replacement. */
import { useState } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import {
  beginSend,
  failSend,
} from "../../../../src/conversation/application/usecases/send-draft"
import { emptyLocalTabs } from "../../../../src/conversation/application/local-tabs"
import { conversation, textContent } from "../../../../src/conversation/model"
import { applyView } from "../../../../src/conversation/application/usecases/apply-view"
import { Transcript } from "../../../../src/conversation/ui/transcript"
import { view } from "../../../../src/desktop/workspace/adapters/gateway/fake-gateway"
import "@fontsource-variable/geist"
import "../../../../src/styles.css"

const provider =
  new URLSearchParams(location.search).get("provider") === "codex" ? "codex" : "claude"
const wire = view("panel-auth", {
  messages: [
    {
      executionId: "refused",
      userText: "Hello",
      attachments: [],
      files: [],
      status: "failed",
      authenticationRequired: true,
      parts: [
        {
          kind: "local_notice",
          offset: 0,
          text: "Nessa declined a tool review.",
          noticeId: "review-1",
          toolId: "",
        },
      ],
    },
  ],
})
const value = applyView(conversation("panel-auth"), {
  ...wire,
  runtime: {
    agent: provider,
    model: "model",
    modelName: provider,
    provider,
    workspace: "/tmp",
    contextWindowTokens: 200000,
    reasoning: true,
  },
  capabilities: { ...wire.capabilities, imageInput: wire.capabilities.imageInput },
})
const store = {
  dispatch: () => undefined,
  getState: () => ({}),
  subscribe: () => () => {},
  replaceReducer: () => undefined,
}
let resolve: (() => void) | undefined
let recover = () => {}
let olderFailure = () => {}
let newerFailure = () => {}
let repeatRefusal = () => {}
let newRefusal = () => {}
let localTabs = emptyLocalTabs()
const fixture = {
  calls: [] as string[],
  fail: false,
  release() {
    resolve?.()
    resolve = undefined
  },
  olderFailure() {
    olderFailure()
  },
  newerFailure() {
    newerFailure()
  },
  repeatRefusal() {
    repeatRefusal()
  },
  newRefusal() {
    newRefusal()
  },
  recover() {
    recover()
  },
}
Object.assign(window, { __providerSignIn: fixture })
function PanelTranscript() {
  const [current, setCurrent] = useState(value)
  const refusal = (executionId: string) => ({
    ...wire,
    runtime: {
      agent: provider,
      model: "model",
      modelName: provider,
      provider,
      workspace: "/tmp",
      contextWindowTokens: 200000,
      reasoning: true,
    },
    messages: [
      { ...wire.messages[0]!, executionId, parts: [], userText: "Later wire input" },
    ],
  })
  olderFailure = () => {
    let tabs = beginSend(emptyLocalTabs(), {
      conversationId: "c0",
      executionId: "A",
      actionId: "action-A",
      mode: "queued",
      content: textContent("Older local failure"),
    })
    tabs = failSend(tabs, "c0", "A", "Offline", { kind: "uncertain" })
    localTabs = tabs
    setCurrent(
      applyView(tabs.conversations[0]!, { ...refusal("B"), conversationId: "c0" }),
    )
  }
  newerFailure = () =>
    setCurrent((held) => {
      let tabs = beginSend(
        { ...localTabs, conversations: [held] },
        {
          conversationId: held.id,
          executionId: "C",
          actionId: "action-C",
          mode: "queued",
          content: textContent("Newer local input"),
        },
      )
      tabs = failSend(tabs, held.id, "C", "Offline", { kind: "uncertain" })
      localTabs = tabs
      return tabs.conversations[0]!
    })
  repeatRefusal = () =>
    setCurrent((held) => applyView(held, { ...refusal("B"), conversationId: held.id }))
  newRefusal = () =>
    setCurrent((held) => applyView(held, { ...refusal("D"), conversationId: held.id }))
  recover = () =>
    setCurrent((held) =>
      applyView(held, {
        ...refusal("D"),
        conversationId: held.id,
        pending: [
          {
            executionId: "retry",
            text: "Try again",
            attachments: [],
            files: [],
            mode: "queued",
          },
        ],
      }),
    )
  return (
    <div
      style={{
        display: "flex",
        height: "100vh",
        padding: "20px 12px",
        maxWidth: 700,
        margin: "0 auto",
      }}
    >
      <Transcript
        conversation={current}
        ground="ink"
        animateMount={false}
        streamText={false}
        emptyState={false}
        statusLabel="Ready"
        gatewayAvailable
        onOpenPaste={() => {}}
        canSignInToProvider={async () =>
          new URLSearchParams(location.search).get("login") !== "unsupported"
        }
        onProviderSignIn={async (chosen) => {
          fixture.calls.push(chosen)
          await new Promise<void>((done) => {
            resolve = done
          })
          if (fixture.fail) throw new Error("launch failed")
        }}
      />
    </div>
  )
}
const container = document.getElementById("root")
if (!container) throw new Error("missing root")
createRoot(container).render(
  <Provider store={store as never}>
    <PanelTranscript />
  </Provider>,
)
