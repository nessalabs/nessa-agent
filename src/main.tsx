import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

import { SetupGate } from "./onboarding"
import { App } from "./panel"
import { SessionLifecycle } from "./session"
import { makeStore } from "./store"
import { createDependencies } from "./composition/dependencies"

import { BrowserApplication } from "./composition/browser"
import { hasNativeHost, windowSurface } from "./host"

import { environmentFromVite } from "./env/vite"

const environment = environmentFromVite()
const dependencies = createDependencies({ environment })
const store = makeStore(dependencies)

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

// The host opens setup in its own window; that window paints setup and nothing
// else, and the panel window paints the panel and nothing else. So the setup
// branch is given no children: the panel tree below belongs to the other window
// and mounting it here would start a second session in a window that is closing.
const panel = (
  <Provider store={store}>
    <App
      attachmentResources={dependencies.attachments}
      canChoosePaths={dependencies.canChoosePaths}
      digest={dependencies.digest}
    />
    {dependencies.usesLocalSession && <SessionLifecycle dependencies={dependencies} />}
  </Provider>
)

createRoot(container).render(
  <React.StrictMode>
    {windowSurface() === "setup" ? (
      <SetupGate agents={dependencies.agents} />
    ) : !hasNativeHost() && environment.conversation.backend === "local" ? (
      <BrowserApplication environment={environment} />
    ) : (
      panel
    )}
  </React.StrictMode>,
)
