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
import {
  gatewaySource,
  type GatewayClock,
} from "../../../../src/desktop/workspace/adapters/gateway/gateway-source"
import { fixtureAppPlugin } from "../../../../src/desktop/widgets/app/fixture/fixture-plugin"
import {
  fixtureCallIdentity,
  fixtureResourceUri,
  fixtureServer,
} from "../../../../src/desktop/widgets/app/fixture/fixture-widgets"
import { pageSandbox } from "../../../../src/desktop/widgets/app/adapters/dom/sandbox-origin"
import { readPageContext } from "../../../../src/desktop/widgets/app/adapters/dom/page-context"
import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "../../../../src/desktop/styles.css"

const fast = "fast",
  slow = "slow"
const gateway = fakeGateway()
const approvalModes = view(fast).approvalModes
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
        approvalModes,
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
const clock: GatewayClock = {
  now: () => performance.now(),
  after(ms, run) {
    const timer = window.setTimeout(run, ms)
    return () => window.clearTimeout(timer)
  },
}
const source = gatewaySource({ connect: () => Promise.resolve(client), clock })
await source.transcript(slow)
const dependencies = createDesktopDependencies({ workspace: source })
dependencies.widgets.register(
  fixtureAppPlugin({
    sessionId: fast,
    sandbox: pageSandbox(document),
    timers: clock,
    newId: () => crypto.randomUUID(),
    page: () => readPageContext(document, "web"),
  }),
)
const store = makeDesktopStore(dependencies)
store.dispatch(followWorkspace())
void store.dispatch(loadWorkspace())
Object.assign(window, {
  __messageSync: {
    async app(prefix: "before" | "added" | "removed") {
      const text = {
        offset: 0,
        kind: "text" as const,
        text: "Earlier text",
        toolId: "",
        noticeId: "",
      }
      const part = {
        offset: 2,
        kind: "tool" as const,
        toolId: fixtureCallIdentity.toolId,
        text: "",
        noticeId: "",
      }
      gateway.views.set(
        fast,
        conversationView(
          view(fast, {
            revision: String(revision++),
            approvalModes,
            messages: [
              {
                executionId: fixtureCallIdentity.executionId,
                userText: "Keep this app",
                attachments: [],
                files: [],
                status: "completed",
                parts:
                  prefix === "removed"
                    ? [part]
                    : prefix === "added"
                      ? [text, { ...text, offset: 1, text: "More text" }, part]
                      : [text, part],
              },
            ],
            tools: [
              {
                ...fixtureCallIdentity,
                title: "show_fixture",
                kind: "other",
                status: "completed",
                details: "Three rows",
                input: "{}",
                mcp: {
                  server: fixtureServer,
                  tool: "show_fixture",
                  resourceUri: fixtureResourceUri,
                },
              },
            ],
          }),
          fast,
        ),
      )
      await source.transcript(fast)
    },
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
