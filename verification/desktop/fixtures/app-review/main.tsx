/**
 * The window over a fake gateway (`fake-gateway.ts`) holding one
 * conversation whose turn has ended, with an MCP App's call in it (#436).
 * `app-review.mjs` drives it.
 *
 * What is the window's own: its composition beside a gateway and its apps
 * (`createDesktopDependencies`), the store and its startup, and
 * `DesktopWindow` with the providers `src/desktop/main.tsx` puts around it,
 * retyped here. What differs from the desktop app: a browser host, no app
 * sandbox (the app itself is not drawn, and its card says so), and nothing
 * inspectable.
 *
 * Nothing is waiting until the app calls a tool (`__appReview.call`): the
 * call goes through that composition (`dependencies.ts`) and the source's
 * `appCall` to the fake's `mcp.callTool`. Like the gateway, which opens a
 * review only once the call is admitted and on record (`app_calls.rs`), the
 * fake opens it later: after the window has read the conversation twice
 * since the call, so a window that reads once when the call begins never
 * sees it. Nothing in the list row moves. The person's answer is held to the
 * review: Allow on it answers the call with the server's result, Deny
 * refuses the call, and an answer to anything else leaves the review open
 * and the call waiting. The fake models one call at a time and refuses a
 * second while one waits. Every view passes the client's own validation
 * (`conversationView`) before the fake serves it.
 *
 * The sample workspace has no app reviews: its source scripts the agent's
 * turns, and an app's review is not one.
 */
import type {
  ConversationMessage,
  ConversationPermission,
  ConversationView,
  McpAppsApi,
  McpServersApi,
  ProductSessionReady,
} from "@nessa/client"
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { bounds } from "../../../../packages/nessa-client/src/generated/product"
import { conversationView } from "../../../../packages/nessa-client/src/protocol/conversation-validate"
import { createDesktopDependencies } from "../../../../src/desktop/dependencies"
import { makeDesktopStore } from "../../../../src/desktop/store"
import { DesktopIconFamilyProvider } from "../../../../src/desktop/ui/icons"
import { DesktopWindow } from "../../../../src/desktop/ui/desktop-window"
import { appPluginId, WidgetRegistryProvider } from "../../../../src/desktop/widgets"
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

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "../../../../src/desktop/styles.css"

const conversation = "0b9a3c1e-5d2f-4a7b-8c6d-1e2f3a4b5c6d"
const server = "mcptest"
// The app's own call, which drew it.
const app = { executionId: "run", toolId: "call-1", instanceId: "mount" }

const ended: ConversationMessage = {
  executionId: app.executionId,
  userText: "Show me the stale rows.",
  attachments: [],
  files: [],
  status: "completed",
  // The app's call, then what the agent said of it.
  parts: [
    {
      offset: 0,
      kind: "tool",
      text: "",
      toolId: app.toolId,
      noticeId: "",
    },
    {
      offset: 1,
      kind: "text",
      text: "Here they are.",
      toolId: "",
      noticeId: "",
    },
  ],
}

/** The conversation's view at `revision`, with the app's review when one is open. */
function viewWith(revision: number, review?: ConversationPermission): ConversationView {
  const value = view(conversation, {
    revision: String(revision),
    messages: [ended],
    tools: [
      {
        executionId: app.executionId,
        toolId: app.toolId,
        title: "show_rows",
        kind: "other",
        status: "completed",
        details: "",
        input: "{}",
        mcp: { server, tool: "show_rows", resourceUri: "ui://mcptest/rows.html" },
      },
    ],
    permissions: review ? [review] : [],
    approvalModes: [
      { id: "ask", name: "Ask", description: "Asks before each tool it runs." },
    ],
  })
  // Held to the client's own validation: a view a gateway may send.
  return conversationView(value, conversation)
}

/** The review the gateway opens for a destructive tool an app calls (`app_reviews.rs`). */
function reviewOf(permissionId: string, tool: string): ConversationPermission {
  return {
    executionId: app.executionId,
    permissionId,
    toolId: app.toolId,
    title: `An app asks to run ${tool} on ${server}`,
    toolName: tool,
    argumentsJson: "{}",
    origin: { kind: "app", server, tool },
    options: [
      { id: "allow", label: "Allow", effect: "allow" },
      { id: "deny", label: "Deny", effect: "deny" },
    ],
  }
}

const gateway = fakeGateway()
gateway.rows.set(
  conversation,
  row(conversation, { title: "Clean up the stale rows", preview: "Here they are." }),
)
gateway.views.set(conversation, viewWith(1))

// What became of the app's call: null before the app calls, "waiting", or
// the outcome the app was given.
let settled: string | null = null
let revision = 1
let reviews = 0
let waiting = false

/** Answers the app's call once the person answers its review, as the gateway does. */
function onAnswer(review: ConversationPermission, done: (allowed: boolean) => void) {
  gateway.once("answer", async (normal) => {
    const args = gateway.calls.at(-1)?.args ?? []
    const [to, execution, permission, option] = args
    const ours =
      to === conversation &&
      execution === review.executionId &&
      permission === review.permissionId
    const chosen = review.options.find((each) => each.id === option)
    if (!ours || !chosen) {
      // Not this review's answer: it stays open, and the call waits.
      onAnswer(review, done)
      return normal()
    }
    gateway.views.set(conversation, viewWith(++revision))
    const answered = await normal()
    done(chosen.effect === "allow")
    return answered
  })
}

const mcpApps = {
  callTool: (_conversation: string, _app: unknown, _server: string, tool: string) => {
    if (waiting)
      return Promise.reject(new Error("the fixture models one app call at a time"))
    waiting = true
    return new Promise((resolve, reject) => {
      const review = reviewOf(`app-review-${++reviews}`, tool)
      const readsAtCall = gateway.count("read")
      const opening = window.setInterval(() => {
        if (gateway.count("read") < readsAtCall + 2) return
        window.clearInterval(opening)
        gateway.views.set(conversation, viewWith(++revision, review))
        onAnswer(review, (allowed) => {
          waiting = false
          if (allowed) resolve({ resultJson: '{"content":[]}' })
          else reject(new Error("The person denied this app's call"))
        })
      }, 20)
    })
  },
  releaseApp: () => Promise.resolve(),
} as unknown as McpAppsApi

// Settings' servers: this fixture never opens Settings, so nothing asks them.
const settingsParts = {
  mcpServers: {} as McpServersApi,
  productSession: { grants: [] } as unknown as ProductSessionReady,
}

const dependencies = createDesktopDependencies({
  gateway: () =>
    Promise.resolve(Object.assign(gateway.client, { mcpApps }, settingsParts)),
  // No sandbox: the app itself is not drawn, and its card says so.
  apps: { sandbox: undefined, platform: "web" },
})
const store = makeDesktopStore(dependencies)
store.dispatch(followWorkspace())
void store.dispatch(loadWorkspace())

Object.assign(window, {
  __appReview: {
    /** The app calls `tool`, as its bridge would: through the window's own plugin. */
    call(tool: string) {
      const plugin = dependencies.widgets.plugin(appPluginId(server))
      if (plugin?.kind !== "app") throw new Error("the app's plugin is not registered")
      settled = "waiting"
      void plugin.ports.server
        .callTool({ sessionId: conversation, server, app }, tool, {})
        .then((outcome) => {
          settled = outcome.kind
        })
    },
    snapshot: () => ({
      settled,
      reads: gateway.count("read"),
      // Every answer the gateway was sent, whichever review it named.
      answers: gateway.calls
        .filter((call) => call.method === "answer")
        .map((call) => [...call.args]),
      openReview: gateway.views.get(conversation)?.permissions[0]?.permissionId ?? null,
    }),
    /** A tool's name with no break in it, as long as the gateway allows. */
    longestTool: "x".repeat(bounds.maxMcpNameBytes),
    /** The most bytes a tool's name may have (`maxMcpNameBytes`). */
    toolBound: bounds.maxMcpNameBytes,
  },
})

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")
createRoot(container).render(
  <React.StrictMode>
    <Provider store={store}>
      <WidgetRegistryProvider registry={dependencies.widgets}>
        <ClockProvider now={dependencies.now}>
          <DesktopIconFamilyProvider>
            <DesktopWindow hostKind="browser" browserSurface inspectable={false} />
          </DesktopIconFamilyProvider>
        </ClockProvider>
      </WidgetRegistryProvider>
    </Provider>
  </React.StrictMode>,
)
