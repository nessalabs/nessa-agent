import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { host } from "../host"
import { createDesktopDependencies } from "./dependencies"
import { makeDesktopStore } from "./store"
import { DesktopIconFamilyProvider } from "./ui/icons"
import { DesktopWindow } from "./ui/desktop-window"
import { ClockProvider, followWorkspace, loadWorkspace } from "./workspace"
import {
  ExperimentsProvider,
  experimentSubagents,
  liveExperimentSource,
} from "./experiments"
import { SubagentsProvider } from "./subagents"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

// Composition: the window's outside things, then the store over them, then
// the tree. The store follows the workspace source for the window's life.
const dependencies = createDesktopDependencies()
const store = makeDesktopStore(dependencies)
// The experiments conversations carry, on the same clock as the workspace.
const experiments = liveExperimentSource({
  now: dependencies.now,
  every: (ms, run) => {
    const timer = window.setInterval(run, ms)
    return () => window.clearInterval(timer)
  },
})
store.dispatch(followWorkspace())
void store.dispatch(loadWorkspace())

// A conversation's subagents, read from the experiment it runs.
const subagents = experimentSubagents(experiments, {
  now: dependencies.now,
  after: (ms, run) => void window.setTimeout(run, ms),
})

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

createRoot(container).render(
  <React.StrictMode>
    <Provider store={store}>
      <ClockProvider now={dependencies.now}>
        <ExperimentsProvider source={experiments}>
          <SubagentsProvider source={subagents}>
            {/* Every icon in the window resolves through the family chosen in Settings. */}
            <DesktopIconFamilyProvider>
              <DesktopWindow
                hostKind={host.kind}
                browserSurface={host.kind === "browser"}
                inspectable={import.meta.env.DEV}
              />
            </DesktopIconFamilyProvider>
          </SubagentsProvider>
        </ExperimentsProvider>
      </ClockProvider>
    </Provider>
  </React.StrictMode>,
)
