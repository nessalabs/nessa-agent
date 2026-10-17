import { AgentDownloads } from "../../../../src/onboarding/ui/agent-downloads"
import {
  AgentSetupProvider,
  SelectedAgentSetup,
  type AgentSetupSource,
} from "../../../../src/onboarding/ui/selected-agent-setup"
/** Real conversation/overview recovery controls over a source that certainly refuses sends. */
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { ClipboardProvider } from "../../../../src/conversation"
import { writeClipboardText } from "../../../../src/host/window"
import { ClockProvider } from "../../../../src/clock"
import { createDesktopDependencies } from "../../../../src/desktop/dependencies"
import { makeDesktopStore } from "../../../../src/desktop/store"
import { fakeSource } from "../../../../src/desktop/workspace/testing"
import {
  loadWorkspace,
  openSession,
  sendMessage,
} from "../../../../src/desktop/workspace"
import { WorkspaceSourceError } from "../../../../src/desktop/workspace/application/ports"
import { Conversation } from "../../../../src/desktop/workspace/ui/panes/conversation"
import { ReplyPill } from "../../../../src/desktop/workspace/ui/overview/reply-pill"
import "@fontsource-variable/geist"
import "../../../../src/desktop/styles.css"
import "../../../../src/desktop/workspace/ui/chrome/chrome.css"
import "../../../../src/desktop/workspace/ui/panes/panes.css"

const surface = new URLSearchParams(location.search).get("surface")
const firstUse = surface === "first-use"
const downloads = surface === "downloads"
const setupCase = surface === "setup" || firstUse || downloads
let readiness: "not-installed" | "needs-authentication" | "ready" = "not-installed"
const state = {
  attempts: 0,
  copied: "",
  diagnostic:
    "Provider refused the request.\nFull technical details stay available for copying.",
  code: setupCase ? "agent_not_configured" : "model_unavailable",
  installed: false,
  installs: [] as string[],
  signIns: 0,
  releaseInstall: () => {},
}
Object.assign(window, { __sendFailure: state })
const source = fakeSource()
const originalSend = source.send
source.send = async (message) => {
  state.attempts++
  if (setupCase && readiness === "ready") return originalSend(message)
  throw new WorkspaceSourceError(
    setupCase ? "agent-not-configured" : "not-supported",
    undefined,
    {
      message: state.diagnostic,
      code: state.code,
    },
  )
}
const setup: AgentSetupSource | undefined = setupCase
  ? {
      readiness: { read: async () => ({ ok: true, agents: { claude: readiness } }) },
      installations: {
        subscribe: () => () => {},
        offers: async () => [
          { agent: "claude", installed: state.installed, archiveBytes: 117_000_000 },
        ],
        install: async (agent) => {
          state.installs.push(agent)
          await new Promise<void>((resolve) => {
            state.releaseInstall = resolve
          })
          state.installed = true
          readiness = "needs-authentication"
          return { status: "installed", cleanupPending: false }
        },
      },
      loginAvailable: async () => true,
      signIn: async () => {
        state.signIns++
        readiness = "ready"
      },
    }
  : undefined
const store = makeDesktopStore(
  createDesktopDependencies({ workspace: source, now: () => 1000 }),
)
await store.dispatch(loadWorkspace())
store.dispatch(openSession({ sessionId: "b" }))
if (!firstUse && !downloads)
  await store.dispatch(
    sendMessage({ sessionId: "b", text: "Hello", initiator: "person" }),
  )
const root = document.getElementById("root")
if (!root) throw new Error("missing root")
root.dataset.surface = "window"
root.style.height = "100vh"
const overview = new URLSearchParams(location.search).get("surface") === "overview"
createRoot(root).render(
  <AgentSetupProvider source={setup}>
    <Provider store={store}>
      <ClockProvider now={() => 1000}>
        <ClipboardProvider
          write={async (text) => {
            if (new URLSearchParams(location.search).get("clipboard") === "native")
              await writeClipboardText(text)
            state.copied = text
          }}
        >
          <div className="workspace" style={{ height: "100%", padding: 16, minWidth: 0 }}>
            <article
              className="workspace-pane"
              data-pane-key="fixture"
              style={{ width: "100%", height: "100%" }}
            >
              <div className="workspace-pane-body">
                {downloads && setup ? (
                  <AgentDownloads source={setup.installations} />
                ) : firstUse ? (
                  <SelectedAgentSetup agent="claude" />
                ) : overview ? (
                  <div style={{ marginTop: "auto", padding: 20 }}>
                    <ReplyPill sessionId="b" agent="Claude" onLeave={() => {}} />
                  </div>
                ) : (
                  <Conversation sessionId="b" onHeadingVisible={() => {}} />
                )}
              </div>
            </article>
          </div>
        </ClipboardProvider>
      </ClockProvider>
    </Provider>
  </AgentSetupProvider>,
)
