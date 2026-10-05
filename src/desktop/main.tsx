import type { NessaClient } from "@nessa/client"
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { host, signInToProvider, providerLoginAvailable } from "../host"
import { environmentFromVite } from "../env/vite"
import { connectBrowserSession, createBrowserAuth } from "../session"
import { createDesktopDependencies } from "./dependencies"
import { hostGateway } from "./adapters/host-gateway"
import { workspaceBackend, type WorkspaceBackend } from "./model/workspace-backend"
import { LinkedDevicesProvider, McpServersProvider } from "./settings"
import { makeDesktopStore } from "./store"
import { DesktopIconFamilyProvider } from "./ui/icons"
import { DesktopWindow } from "./ui/desktop-window"
import { platformFor, sandboxFor, WidgetRegistryProvider } from "./widgets"
import { ClockProvider, followWorkspace, loadWorkspace } from "./workspace"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

document.documentElement.dataset.nessaModule = "started"

// Composition: the window's outside things and its widget plugins, then the
// store over them, then the tree. The store follows the workspace source for
// the window's life. Where the workspace comes from is `workspaceBackend`'s
// to say: the desktop app's own gateway, over the credential its host hands
// the panel (`adapters/host-gateway.ts`); a browser preview opened
// with `?gateway`, over the session this origin signed in to; otherwise the
// sample (`model/workspace-backend.ts`).
const environment = environmentFromVite()
const gateway = windowGateway(workspaceBackend(host.kind, window.location.search))

function windowGateway(
  backend: WorkspaceBackend,
): (() => Promise<NessaClient>) | undefined {
  switch (backend) {
    case "host":
      return hostGateway(environment)
    case "browser": {
      // This origin's sign-in, as the panel's browser surface keeps it.
      // Storage is reached at each use, so storage the browser blocks is
      // `createBrowserAuth`'s to survive rather than a throw before the
      // window renders.
      const auth = createBrowserAuth(window.fetch.bind(window), {
        getItem: (key) => window.sessionStorage.getItem(key),
        setItem: (key, value) => window.sessionStorage.setItem(key, value),
        removeItem: (key) => window.sessionStorage.removeItem(key),
      })
      return () =>
        connectBrowserSession({
          auth,
          stage: environment.stage,
          clientId: "nessa-browser",
          surfaceKind: "desktop",
          pageUrl: window.location.href,
        }).then((session) => session.client)
    }
    case "sample":
      return undefined
  }
}
const dependencies = createDesktopDependencies({
  signInToProvider,
  providerLoginAvailable,
  gateway,
  apps: {
    sandbox: sandboxFor(host.kind, document),
    platform: platformFor(host.kind),
  },
})
const store = makeDesktopStore(dependencies)
store.dispatch(followWorkspace())
void store.dispatch(loadWorkspace())

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

document.documentElement.dataset.nessaMounted = "1"
createRoot(container).render(
  <React.StrictMode>
    <Provider store={store}>
      <WidgetRegistryProvider registry={dependencies.widgets}>
        <ClockProvider now={dependencies.now}>
          {/* Every icon in the window resolves through the family chosen in Settings. */}
          <DesktopIconFamilyProvider>
            <McpServersProvider gateway={dependencies.mcpServers}>
              <LinkedDevicesProvider gateway={dependencies.linkedDevices}>
                <DesktopWindow
                  hostKind={host.kind}
                  browserSurface={host.kind === "browser"}
                  inspectable={import.meta.env.DEV}
                />
              </LinkedDevicesProvider>
            </McpServersProvider>
          </DesktopIconFamilyProvider>
        </ClockProvider>
      </WidgetRegistryProvider>
    </Provider>
  </React.StrictMode>,
)
