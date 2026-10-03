/**
 * The real window — its gateway source, views, store and UI — over a fake
 * gateway (`fake-gateway.ts`) whose one conversation has ended its turn and
 * waits on a review an MCP App asked for (#436). `app-review.mjs` drives it.
 * The sample workspace has no app reviews: its source scripts the agent's
 * turns, and an app's review is not one.
 */
import type { ConversationPermission, McpAppsApi } from "@nessa/client"
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { createDesktopDependencies } from "../../../../src/desktop/dependencies"
import { makeDesktopStore } from "../../../../src/desktop/store"
import { DesktopIconFamilyProvider } from "../../../../src/desktop/ui/icons"
import { DesktopWindow } from "../../../../src/desktop/ui/desktop-window"
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

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "../../../../src/desktop/styles.css"

const conversation = "app-review"
const params = new URL(location.href).searchParams
// A tool's name is one word, as long as its server makes it (`?tool=`).
const tool = params.get("tool") ?? "app_delete_row"

const review: ConversationPermission = {
  executionId: "turn",
  permissionId: "app-1",
  toolId: "app-call-1",
  title: `An app asks to run ${tool} on mcptest`,
  toolName: tool,
  argumentsJson: "{}",
  origin: { kind: "app", server: "mcptest", tool },
  options: [
    { id: "allow", label: "Allow", effect: "allow" },
    { id: "deny", label: "Deny", effect: "deny" },
  ],
}

const gateway = fakeGateway()
gateway.rows.set(
  conversation,
  row(conversation, { title: "Clean up the stale rows", preview: "Done." }),
)
gateway.views.set(
  conversation,
  view(conversation, {
    revision: "1",
    messages: [
      {
        executionId: "turn",
        userText: "Clean up the stale rows.",
        attachments: [],
        files: [],
        status: "completed",
        parts: [],
      },
    ],
    permissions: [review],
  }),
)

// No app is drawn here (no `apps`), so the window never reaches its servers.
const client = Object.assign(gateway.client, { mcpApps: {} as McpAppsApi })
const dependencies = createDesktopDependencies({ gateway: () => Promise.resolve(client) })
const store = makeDesktopStore(dependencies)
store.dispatch(followWorkspace())
void store.dispatch(loadWorkspace())

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
