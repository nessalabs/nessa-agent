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
// else, and the panel window paints the panel and nothing else.
const panel = (
  <Provider store={store}>
    <App attachmentResources={dependencies.attachments} />
    {dependencies.usesLocalSession && <SessionLifecycle dependencies={dependencies} />}
  </Provider>
)

createRoot(container).render(
  <React.StrictMode>
    {windowSurface() === "setup" ? (
      <SetupGate>{panel}</SetupGate>
    ) : !hasNativeHost() && environment.conversation.backend === "local" ? (
      <BrowserApplication environment={environment} />
    ) : (
      panel
    )}
  </React.StrictMode>,
)
