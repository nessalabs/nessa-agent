/**
 * The real window — composed as `src/desktop/main.tsx` composes it beside a
 * gateway and its apps — over a fake gateway (`fake-gateway.ts`) holding one
 * conversation whose turn has ended, with an MCP App's call in it (#436).
 * `app-review.mjs` drives it.
 *
 * Nothing is waiting until the app calls a tool (`__appReview.call`): the
 * call goes through the window's composition (`dependencies.ts`) and the
 * source's `appCall` to the fake's `mcp.callTool`, which opens the app's
 * review in the conversation's view, as the gateway does, and answers the
 * call once the person answers the review. Nothing in the list row moves, so
 * the review is drawn only if the source reads the conversation while the
 * call waits. Every view is held to the client's own validation
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
} from "@nessa/client"
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
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
  // As the client would refuse it, so the fixture is a view a gateway may send.
  return conversationView(value, conversation)
}

/** The review the gateway opens for a destructive tool an app calls (`app_reviews.rs`). */
function reviewOf(tool: string): ConversationPermission {
  return {
    executionId: app.executionId,
    permissionId: `app-review-${tool.length}`,
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

// What became of the app's call: unanswered, or how it was answered.
let settled: string | null = null
let revision = 1
const mcpApps = {
  callTool: (_conversation: string, _app: unknown, _server: string, tool: string) =>
    new Promise((resolve) => {
      gateway.views.set(conversation, viewWith(++revision, reviewOf(tool)))
      gateway.once("answer", async (normal) => {
        gateway.views.set(conversation, viewWith(++revision))
        const answered = await normal()
        resolve({ resultJson: '{"content":[]}' })
        return answered
      })
    }),
  releaseApp: () => Promise.resolve(),
} as unknown as McpAppsApi

const dependencies = createDesktopDependencies({
  gateway: () => Promise.resolve(Object.assign(gateway.client, { mcpApps })),
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
      answers: gateway.count("answer"),
    }),
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
