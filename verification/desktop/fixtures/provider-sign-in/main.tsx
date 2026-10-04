/** The real transcript over typed auth-required evidence and a controllable login port. */
import { createRef } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { createDesktopDependencies } from "../../../../src/desktop/dependencies"
import { makeDesktopStore } from "../../../../src/desktop/store"
import {
  ClockProvider,
  followWorkspace,
  loadWorkspace,
  openSession,
} from "../../../../src/desktop/workspace"
import { fakeSource, testIndex } from "../../../../src/desktop/workspace/testing"
import { transcriptFrom } from "../../../../src/desktop/workspace/adapters/gateway/gateway-views"
import { view } from "../../../../src/desktop/workspace/adapters/gateway/fake-gateway"
import { emptyTranscript } from "../../../../src/desktop/workspace/model/transcript"
import { Transcript } from "../../../../src/desktop/workspace/ui/transcript/transcript"
import "@fontsource-variable/geist"
import "../../../../src/desktop/styles.css"
import "../../../../src/desktop/workspace/ui/chrome/chrome.css"

const provider =
  new URLSearchParams(window.location.search).get("provider") === "codex"
    ? "codex"
    : "claude"
const index = testIndex()
const source = fakeSource({
  ...index,
  sessions: index.sessions.map((session) =>
    session.id === "b"
      ? {
          ...session,
          model:
            provider === "claude"
              ? { provider: "anthropic", modelId: "claude-opus-5" }
              : { provider: "openai", modelId: "gpt-6-astra" },
        }
      : session,
  ),
})
source.transcripts.set(
  "b",
  transcriptFrom(
    view("b", {
      messages: [
        {
          executionId: "refused",
          userText: "Hello",
          attachments: [],
          files: [],
          status: "failed",
          authenticationRequired: true,
          error: "Internal error: OAuth session expired",
          parts: [
            {
              kind: "local_notice",
              offset: 0,
              text: "Nessa declined a tool review.",
              noticeId: "review-1",
              toolId: "",
            },
          ],
        },
      ],
    }),
    1,
    () => 1000,
  ),
)

let resolve: (() => void) | undefined
const fixture = {
  calls: [] as string[],
  fail: false,
  release() {
    resolve?.()
    resolve = undefined
  },
  recover() {
    source.emit({
      kind: "transcript",
      transcript: { ...emptyTranscript("b"), revision: 2 },
    })
  },
}
Object.assign(window, { __providerSignIn: fixture })
const dependencies = createDesktopDependencies({
  workspace: source,
  providerLoginAvailable: async () =>
    new URLSearchParams(location.search).get("login") !== "unsupported",
  signInToProvider: async (chosen) => {
    fixture.calls.push(chosen)
    await new Promise<void>((done) => {
      resolve = done
    })
    if (fixture.fail) throw new Error("launch failed")
  },
})
const store = makeDesktopStore(dependencies)
store.dispatch(followWorkspace())
await store.dispatch(loadWorkspace())
store.dispatch(openSession({ sessionId: "b" }))
const container = document.getElementById("root")
if (!container) throw new Error("missing root")
container.dataset.surface = "window"
container.style.height = "100vh"
container.style.display = "flex"
createRoot(container).render(
  <Provider store={store}>
    <ClockProvider now={() => 1000}>
      <Transcript
        sessionId="b"
        arriving={false}
        scrollRef={createRef()}
        headingRef={createRef()}
        onHeadingVisible={() => {}}
      />
    </ClockProvider>
  </Provider>,
)
