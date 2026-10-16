/**
 * The panel's real refusal notice over a store with two refused messages, the
 * newer one sent again and accepted. Send Again on the older one must send it.
 */
import { createElement } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { createDependencies } from "../../../../src/composition/dependencies"
import { makeStore } from "../../../../src/store"
import { scenarioEffects, sendDraft } from "../../../../src/conversation/testing"
import { textContent } from "../../../../src/conversation/model"
import { ConversationNotification } from "../../../../src/conversation/ui/conversation-notification"
import "../../../../src/styles.css"

const effects = scenarioEffects("echo")
const state = { refusals: 2, submitted: [] as string[] }
Object.assign(window, { __refused: state })
const store = makeStore(
  createDependencies({
    conversation: {
      ...effects,
      create: async (...args: Parameters<typeof effects.create>) => {
        if (state.refusals-- > 0) throw new Error("offline")
        return effects.create(...args)
      },
      submit: async (input: Parameters<typeof effects.submit>[0]) => {
        state.submitted.push(input.text)
        return effects.submit(input)
      },
    },
  }),
)
const tab = () => {
  const found = store.getState().conversation.conversations[0]
  if (!found) throw new Error("missing conversation")
  return found
}
await store.dispatch(sendDraft({ content: textContent("first") }))
const id = tab().id
await store.dispatch(sendDraft({ id, content: textContent("second") }))
const newer = tab().turns[1]
if (!newer) throw new Error("missing newer refusal")
await store.dispatch(
  sendDraft({ id, content: textContent("second"), retryTurnId: newer.id }),
)

const root = document.getElementById("root")
if (!root) throw new Error("missing root")
root.dataset.surface = "panel"
const view = createRoot(root)
const render = () =>
  view.render(
    createElement(Provider, {
      store,
      children: createElement(
        "main",
        { style: { padding: 16 } },
        createElement(ConversationNotification, {
          conversation: tab(),
          connection: { phase: "ready", detail: "", retry: () => {} },
          gatewayAvailable: true,
        }),
        createElement("output", {
          "data-turns": tab()
            .turns.map((turn) => (turn.from === "user" ? turn.receipt : turn.from))
            .join(","),
        }),
      ),
    }),
  )
store.subscribe(render)
render()
