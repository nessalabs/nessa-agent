/** Production source/store/window; controlled gateway responses measure delivery only. */
import * as React from "react"
import type { ConversationListResult, ConversationView } from "@nessa/client"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { conversationView } from "../../../../packages/nessa-client/src/protocol/conversation-validate"
import { createDesktopDependencies } from "../../../../src/desktop/dependencies"
import { makeDesktopStore } from "../../../../src/desktop/store"
import { DesktopWindow } from "../../../../src/desktop/ui/desktop-window"
import { DesktopIconFamilyProvider } from "../../../../src/desktop/ui/icons"
import { WidgetRegistryProvider } from "../../../../src/desktop/widgets"
import {
  ClockProvider,
  followWorkspace,
  loadWorkspace,
} from "../../../../src/desktop/workspace"
import {
  deferred,
  fakeGateway,
  row,
  view,
} from "../../../../src/desktop/workspace/adapters/gateway/fake-gateway"
import { gatewaySource } from "../../../../src/desktop/workspace/adapters/gateway/gateway-source"
import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "../../../../src/desktop/styles.css"

const fast = "fast",
  slow = "slow"
const gateway = fakeGateway()
let revision = 1,
  holdList = false,
  holdRead = false
let lists = 0,
  reads = 0,
  fastReads = 0,
  slowReads = 0,
  heldLists = 0,
  heldReads = 0
const listReply = deferred<ConversationListResult>()
const readReply = deferred<ConversationView>()
function changed(text: string, running = true) {
  gateway.views.set(
    fast,
    conversationView(
      view(fast, {
        revision: String(revision++),
        approvalModes: [{ id: "ask", name: "Ask", description: "Ask before tools." }],
        messages: [
          {
            executionId: "turn",
            userText: "Synchronize this message",
            attachments: [],
            files: [],
            status: running ? "running" : "completed",
            parts: [{ offset: 0, kind: "text", text, toolId: "", noticeId: "" }],
          },
        ],
      }),
      fast,
    ),
  )
}
gateway.rows.set(fast, row(fast, { title: "Message synchronization" }))
gateway.rows.set(slow, row(slow, { title: "Slow conversation", running: true }))
gateway.views.set(slow, view(slow))
changed("Initial answer", false)
let lastFastRequest: { at: number; revision: string; text: string } | null = null
const client = {
  ...gateway.client,
  conversation: {
    ...gateway.client.conversation,
    list: () => {
      lists++
      if (holdList) {
        heldLists++
        return listReply.promise
      }
      return gateway.client.conversation.list()
    },
    read: (id: string) => {
      reads++
      if (id === fast) fastReads++
      else slowReads++
      if (holdRead && id === slow) {
        heldReads++
        return readReply.promise
      }
      if (id === fast) {
        const requested = gateway.views.get(id)!
        lastFastRequest = {
          at: performance.now(),
          revision: requested.revision,
          text: requested.messages
            .flatMap((message) =>
              message.parts
                .filter((part) => part.kind === "text")
                .map((part) => part.text),
            )
            .join("\n"),
        }
      }
      return gateway.client.conversation.read(id)
    },
  },
}
const source = gatewaySource({
  connect: () => Promise.resolve(client),
  clock: {
    now: () => performance.now(),
    after(ms, run) {
      const timer = window.setTimeout(run, ms)
      return () => window.clearTimeout(timer)
    },
  },
})
await source.transcript(slow)
const dependencies = createDesktopDependencies({ workspace: source })
const store = makeDesktopStore(dependencies)
store.dispatch(followWorkspace())
void store.dispatch(loadWorkspace())
Object.assign(window, {
  __messageSync: {
    async start() {
      changed("Initial answer")
      await source.send({
        sessionId: fast,
        messageId: "turn",
        text: "Synchronize this message",
        model: { provider: "anthropic", modelId: "claude-opus-5" },
        initiator: "person",
      })
    },
    publish(text: string) {
      changed(text)
      return performance.now()
    },
    hold() {
      holdList = true
      holdRead = true
    },
    snapshot() {
      return {
        lists,
        reads,
        fastReads,
        slowReads,
        heldLists,
        heldReads,
        now: performance.now(),
        lastFastRequest,
      }
    },
    rest() {
      holdList = false
      holdRead = false
      gateway.rows.set(fast, row(fast, { title: "Message synchronization" }))
      gateway.rows.set(slow, row(slow, { title: "Slow conversation" }))
      changed("Finished", false)
      gateway.views.set(slow, view(slow, { revision: "finished" }))
      listReply.resolve({ conversations: [...gateway.rows.values()], complete: true })
      readReply.resolve(gateway.views.get(slow)!)
    },
  },
})
const container = document.getElementById("root")
if (!container) throw new Error("missing #root")
createRoot(container).render(
  <Provider store={store}>
    <WidgetRegistryProvider registry={dependencies.widgets}>
      <ClockProvider now={dependencies.now}>
        <DesktopIconFamilyProvider>
          <DesktopWindow hostKind="browser" browserSurface inspectable={false} />
        </DesktopIconFamilyProvider>
      </ClockProvider>
    </WidgetRegistryProvider>
  </Provider>,
)
