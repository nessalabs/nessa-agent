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
  signInToProvider,
  providerLoginAvailable,
  hostStartup,
  quitNessa,
  restartNessa,
  windowSize,
  windowSurface,
} from "./host"
import { StartupRefused } from "./startup"
import { publishWindowSize } from "./panel/adapters/panel-frame"

import { environmentFromVite } from "./env/vite"
import { installDevConsoleForwarding } from "./diagnostics/dev-console"

document.documentElement.dataset.nessaModule = "started"

if (import.meta.env.DEV) installDevConsoleForwarding()

const environment = environmentFromVite()
const dependencies = createDependencies({ environment })
const store = makeStore(dependencies)

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

// The host opens setup in its own window; that window paints setup and nothing
// else, and the panel window paints the panel and nothing else. So the setup
// branch is given no children: the panel tree below belongs to the other window
// and mounting it here would create a panel in a window that is closing.
// Setup owns an authenticated session for explicit native-runtime downloads.
const panel = (
  <Provider store={store}>
    <App
      onProviderSignIn={signInToProvider}
      canSignInToProvider={providerLoginAvailable}
      attachmentResources={dependencies.attachments}
      canChoosePaths={dependencies.canChoosePaths}
      digest={dependencies.digest}
      loadConversationChoices={dependencies.loadConversationChoices}
      agentInstallations={dependencies.agentInstallations}
    />
    {dependencies.usesLocalSession && <SessionLifecycle dependencies={dependencies} />}
  </Provider>
)

const root = createRoot(container)

// The load fallback in index.html stays on screen until hostStartup answers;
// with the window's size it centres in the panel instead of its corner box.
if (windowSurface() === "panel")
  void windowSize().then(publishWindowSize, () => undefined)

// Asked before anything is mounted (ADR 221): a host that could not put itself
// together answers nothing else, so the panel below would only fail in pieces.
// An unanswerable question is treated as ready, which is what this page did
// before it could ask.
void hostStartup()
  .catch(() => ({ state: "ready" }) as const)
  .then((startup) => {
    if (startup.state === "refused") {
      document.documentElement.dataset.nessaMounted = "1"
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
  document.documentElement.dataset.nessaMounted = "1"
  root.render(
    <React.StrictMode>
      {windowSurface() === "setup" ? (
        <Provider store={store}>
          <SetupGate
            agents={dependencies.agents}
            apiKeys={nativeAgentApiKeys}
            installations={dependencies.agentInstallations}
          />
          <SessionLifecycle dependencies={dependencies} />
        </Provider>
      ) : !hasNativeHost() && environment.conversation.backend === "local" ? (
        <BrowserApplication environment={environment} />
      ) : (
        panel
      )}
    </React.StrictMode>,
  )
}
