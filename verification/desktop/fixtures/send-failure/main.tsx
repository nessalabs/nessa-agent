/** Real conversation/overview recovery controls over a source that certainly refuses sends. */
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { ClipboardProvider } from "../../../../src/conversation"
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

const state = {
  attempts: 0,
  copied: "",
  diagnostic:
    "Provider refused the request.\nFull technical details stay available for copying.",
  code: "model_unavailable",
}
Object.assign(window, { __sendFailure: state })
const source = fakeSource()
source.send = async () => {
  state.attempts++
  throw new WorkspaceSourceError("not-supported", undefined, {
    message: state.diagnostic,
    code: state.code,
  })
}
const store = makeDesktopStore(
  createDesktopDependencies({ workspace: source, now: () => 1000 }),
)
await store.dispatch(loadWorkspace())
store.dispatch(openSession({ sessionId: "b" }))
await store.dispatch(sendMessage({ sessionId: "b", text: "Hello", initiator: "person" }))
const root = document.getElementById("root")
if (!root) throw new Error("missing root")
root.dataset.surface = "window"
root.style.height = "100vh"
const overview = new URLSearchParams(location.search).get("surface") === "overview"
createRoot(root).render(
  <Provider store={store}>
    <ClockProvider now={() => 1000}>
      <ClipboardProvider
        write={async (text) => {
          state.copied = text
        }}
      >
        <div className="workspace" style={{ height: "100%", padding: 16 }}>
          <article
            className="workspace-pane"
            data-pane-key="fixture"
            style={{ width: "100%", height: "100%" }}
          >
            <div className="workspace-pane-body">
              {overview ? (
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
  </Provider>,
)
