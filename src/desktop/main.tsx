import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { host } from "../host"
import { createDesktopDependencies } from "./dependencies"
import { makeDesktopStore } from "./store"
import { DesktopIconFamilyProvider } from "./ui/icons"
import { DesktopWindow } from "./ui/desktop-window"
import { WidgetRegistryProvider } from "./widgets"
import { ClockProvider, followWorkspace, loadWorkspace } from "./workspace"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

// Composition: the window's outside things and its widget plugins, then the
// store over them, then the tree. The store follows the workspace source for
// the window's life.
const dependencies = createDesktopDependencies()
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
