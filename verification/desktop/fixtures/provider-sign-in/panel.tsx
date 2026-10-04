/** The floating panel's real transcript, including its typed error-divider replacement. */
import { useState } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { conversation } from "../../../../src/conversation/model"
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
const fixture = {
  calls: [] as string[],
  fail: false,
  release() {
    resolve?.()
    resolve = undefined
  },
  recover() {
    recover()
  },
}
Object.assign(window, { __providerSignIn: fixture })
function PanelTranscript() {
  const [current, setCurrent] = useState(value)
  recover = () =>
    setCurrent({
      ...value,
      turns: [
        ...value.turns,
        {
          id: "new-prompt",
          from: "user",
          executionId: "retry",
          receipt: "delivered",
          content: [{ type: "text", text: "Try again" }],
        },
      ],
    })
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
