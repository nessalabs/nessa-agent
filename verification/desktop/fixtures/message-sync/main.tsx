/**
 * Production source/store/window over controlled gateway subscriptions, to
 * measure delivery only: a frame of the fast conversation, sent when the test
 * publishes, to the text on screen. `hold` holds back the list's and the slow
 * conversation's frames, as a gateway that cannot send them yet would.
 */
import * as React from "react"
import type { GatewayClient } from "../../../../src/desktop/workspace/adapters/gateway/gateway-source"
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
  holding = false
let subscribes = 0,
  lists = 0,
  fastFrames = 0,
  slowFrames = 0,
  heldLists = 0,
  heldReads = 0
// Frames held back while `holding`, each handed on by `rest`.
const heldFrames: (() => void)[] = []
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
let lastFastFrame: { at: number; revision: string; text: string } | null = null
const client: GatewayClient = {
  ...gateway.client,
  subscriptions: {
    view: (id, handlers, options) => {
      subscribes++
      return gateway.client.subscriptions.view(
        id,
        {
          view: (frame) => {
            if (id === fast) {
              fastFrames++
              lastFastFrame = {
                at: performance.now(),
                revision: frame.view.revision,
                text: frame.view.messages
                  .flatMap((message) =>
                    message.parts
                      .filter((part) => part.kind === "text")
                      .map((part) => part.text),
                  )
                  .join("\n"),
              }
              return handlers.view(frame)
            }
            slowFrames++
            if (!holding) return handlers.view(frame)
            heldReads++
            heldFrames.push(() => handlers.view(frame))
          },
          ended: handlers.ended,
        },
        options,
      )
    },
    list: (handlers, options) => {
      subscribes++
      return gateway.client.subscriptions.list(
        {
          list: (list) => {
            lists++
            if (!holding) return handlers.list(list)
            heldLists++
            heldFrames.push(() => handlers.list(list))
          },
          ended: handlers.ended,
        },
        options,
      )
    },
  },
  get connectionState() {
    return gateway.client.connectionState
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
      gateway.publish(fast)
      await source.transcript(fast)
    },
    async start() {
      changed("Initial answer")
      gateway.publish(fast)
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
      const at = performance.now()
      gateway.publish(fast)
      return at
    },
    hold() {
      holding = true
      gateway.rows.set(
        slow,
        row(slow, { title: "Slow conversation", updatedAtMs: 3_000 }),
      )
      gateway.publishList()
      gateway.views.set(slow, view(slow, { revision: "held" }))
      gateway.publish(slow)
    },
    snapshot() {
      return {
        subscribes,
        lists,
        fastFrames,
        slowFrames,
        heldLists,
        heldReads,
        observes: gateway.count("observe"),
        now: performance.now(),
        lastFastFrame,
      }
    },
    rest() {
      holding = false
      for (const frame of heldFrames.splice(0)) frame()
      gateway.rows.set(fast, row(fast, { title: "Message synchronization" }))
      gateway.rows.set(slow, row(slow, { title: "Slow conversation" }))
      changed("Finished", false)
      gateway.publish(fast)
      gateway.views.set(slow, view(slow, { revision: "finished" }))
      gateway.publish(slow)
      gateway.publishList()
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
