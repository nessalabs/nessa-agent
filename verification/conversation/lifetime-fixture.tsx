/** Production follow/control consumers over deterministic injected command ports. */
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { makeStore } from "../../src/store"
import { createDependencies } from "../../src/composition/dependencies"
import { sessionReady } from "../../src/session/testing"
import { scenarioEffects } from "../../src/conversation/adapters/scenario/effects"
import {
  controlConversation,
  restoreConversations,
} from "../../src/conversation/adapters/store/slice"
import { useConversation } from "../../src/conversation/ui/use-conversation"
import { ConversationControls } from "../../src/conversation/ui/conversation-controls"
import { ClockProvider } from "../../src/clock"
import { createLifetimeCounters, publishLifetimeCounters } from "./lifetime-counters"

const A = "00000000-0000-4000-8000-000000000001"
const B = "00000000-0000-4000-8000-000000000002"
const counters = createLifetimeCounters()
publishLifetimeCounters(counters)
let release!: () => void
const preparation = new Promise<void>((done) => {
  release = done
})
const store = makeStore(
  createDependencies({
    conversation: {
      ...scenarioEffects("echo"),
      create: async (conversationId) => {
        await preparation
        return { conversationId }
      },
      close: async (conversationId) => {
        counters.noteClose(conversationId)
      },
      follow: (id, follower) => {
        counters.noteFollow(id)
        queueMicrotask(() => follower.failed("unavailable"))
        return () => {
          counters.noteStop()
        }
      },
    },
  }),
)
store.dispatch(
  sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
)
store.dispatch(restoreConversations({ tabs: [{ conversationId: A, title: "Original" }] }))

function Fixture() {
  const { active } = useConversation()
  const [phase, setPhase] = React.useState("ready")
  const pending = React.useRef<Promise<unknown> | undefined>(undefined)
  return (
    <main
      data-local-lifetimes
      data-phase={phase}
      data-local-id={active.id}
      data-server-id={active.serverConversationId}
      data-error={active.error ?? ""}
    >
      <h1>{active.title}</h1>
      <p>Local tab: {active.id}</p>
      <p>Remote conversation: {active.serverConversationId}</p>
      <button
        onClick={() => {
          pending.current = store.dispatch(
            controlConversation({ id: active.id, control: { kind: "close" } }),
          )
          setPhase("waiting")
        }}
      >
        Start original close
      </button>
      <button
        onClick={() => {
          const same = new URLSearchParams(location.search).has("same")
          store.dispatch(
            restoreConversations({
              tabs: [{ conversationId: same ? A : B, title: "Replacement" }],
            }),
          )
          setPhase("replaced")
        }}
      >
        Restore replacement
      </button>
      <button
        onClick={async () => {
          release()
          await pending.current
          setPhase("finished")
        }}
      >
        Finish original close
      </button>
      <ConversationControls conversation={active} gatewayAvailable />
    </main>
  )
}
const host = document.getElementById("root")
if (!host) throw new Error("Lifetime fixture root missing")
createRoot(host).render(
  <Provider store={store}>
    <ClockProvider now={() => 1_000}>
      <Fixture />
    </ClockProvider>
  </Provider>,
)
