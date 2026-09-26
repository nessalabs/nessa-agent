import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

import { SetupGate } from "./onboarding"
import { nativeAgentApiKeys } from "./onboarding/adapters/agent-api-key"
import { App } from "./panel"
import { SessionLifecycle } from "./session"
import { makeStore } from "./store"
import { createDependencies } from "./composition/dependencies"

import { BrowserApplication } from "./composition/browser"
import {
  hasNativeHost,
  hostStartup,
  quitNessa,
  restartNessa,
  windowSurface,
} from "./host"
import { StartupRefused } from "./startup"

import { environmentFromVite } from "./env/vite"
import { installDevConsoleForwarding } from "./diagnostics/dev-console"

if (import.meta.env.DEV) installDevConsoleForwarding()

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

const root = createRoot(container)

// Asked before anything is mounted (ADR 221): a host that could not put itself
// together answers nothing else, so the panel below would only fail in pieces.
// An unanswerable question is treated as ready, which is what this page did
// before it could ask.
void hostStartup()
  .catch(() => ({ state: "ready" }) as const)
  .then((startup) => {
    if (startup.state === "refused") {
      root.render(
        <React.StrictMode>
          <StartupRefused
            details={startup.details}
            onTryAgain={() => void restartNessa()}
            onQuit={() => void quitNessa()}
          />
        </React.StrictMode>,
      )
      return
    }
    renderApplication()
  })

function renderApplication() {
  root.render(
    <React.StrictMode>
      {windowSurface() === "setup" ? (
        <SetupGate agents={dependencies.agents} apiKeys={nativeAgentApiKeys} />
      ) : !hasNativeHost() && environment.conversation.backend === "local" ? (
        <BrowserApplication environment={environment} />
      ) : (
        panel
      )}
    </React.StrictMode>,
  )
}
