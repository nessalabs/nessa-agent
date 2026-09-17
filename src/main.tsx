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
import { hasNativeHost } from "./host"

import { environmentFromVite } from "./env/vite"

const environment = environmentFromVite()
const dependencies = createDependencies({ environment })
const store = makeStore(dependencies)

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

createRoot(container).render(
  <React.StrictMode>
    {!hasNativeHost() && environment.conversation.backend === "local" ? (
      <BrowserApplication environment={environment} />
    ) : (
      <Provider store={store}>
        <SetupGate>
          <App attachmentResources={dependencies.attachments} />
          {dependencies.usesLocalSession && (
            <SessionLifecycle dependencies={dependencies} />
          )}
        </SetupGate>
      </Provider>
    )}
  </React.StrictMode>,
)
