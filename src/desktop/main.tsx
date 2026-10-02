import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { host } from "../host"
import { environmentFromVite } from "../env/vite"
import { browserSessionUrl, connectDevSession } from "../session"
import { createDesktopDependencies } from "./dependencies"
import { gatewayRequested } from "./model/workspace-backend"
import { makeDesktopStore } from "./store"
import { DesktopIconFamilyProvider } from "./ui/icons"
import { DesktopWindow } from "./ui/desktop-window"
import { platformFor, sandboxFor, WidgetRegistryProvider } from "./widgets"
import { ClockProvider, followWorkspace, loadWorkspace } from "./workspace"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

// Composition: the window's outside things and its widget plugins, then the
// store over them, then the tree. The store follows the workspace source for
// the window's life. A browser preview opened with `?gateway` shows the
// gateway's conversations, over the session this origin signed in to;
// otherwise the window shows the sample (`model/workspace-backend.ts`).
const environment = environmentFromVite()
const gateway =
  host.kind === "browser" && gatewayRequested(window.location.search)
    ? () =>
        connectDevSession({
          stage: environment.stage,
          clientId: "nessa-browser",
          surfaceKind: "desktop",
          browserUrl: browserSessionUrl(window.location.href, environment.stage),
        }).then((session) => session.client)
    : undefined
const dependencies = createDesktopDependencies({
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

createRoot(container).render(
  <React.StrictMode>
    <Provider store={store}>
      <WidgetRegistryProvider registry={dependencies.widgets}>
        <ClockProvider now={dependencies.now}>
          {/* Every icon in the window resolves through the family chosen in Settings. */}
          <DesktopIconFamilyProvider>
            <DesktopWindow
              hostKind={host.kind}
              browserSurface={host.kind === "browser"}
              inspectable={import.meta.env.DEV}
            />
          </DesktopIconFamilyProvider>
        </ClockProvider>
      </WidgetRegistryProvider>
    </Provider>
  </React.StrictMode>,
)
