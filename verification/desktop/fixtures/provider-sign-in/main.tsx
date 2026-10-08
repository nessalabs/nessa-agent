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
  sendMessage,
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
          model: { provider: "anthropic", modelId: "claude-opus-5" },
        }
      : session,
  ),
})
source.transcripts.set(
  "b",
  transcriptFrom(
    view("b", {
      runtime: {
        agent: provider,
        model: "unknown-catalogue-model",
        modelName: "Unknown",
        provider,
        workspace: "/tmp",
        contextWindowTokens: 200000,
        reasoning: true,
      },
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

let retryId: string | undefined
function publishRetry(phase: "queued" | "running" | "completed") {
  if (!retryId) throw new Error("missing retry input")
  source.emit({
    kind: "transcript",
    transcript: transcriptFrom(
      view("b", {
        runtime: {
          agent: provider,
          model: "unknown-catalogue-model",
          modelName: "Unknown",
          provider,
          workspace: "/tmp",
          contextWindowTokens: 200000,
          reasoning: true,
        },
        messages: [
          {
            executionId: "refused",
            userText: "Hello",
            attachments: [],
            files: [],
            status: "failed",
            authenticationRequired: true,
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
          ...(phase === "queued"
            ? []
            : [
                {
                  executionId: retryId,
                  userText: "Try again",
                  attachments: [],
                  files: [],
                  status: phase,
                  parts:
                    phase === "completed"
                      ? [
                          {
                            kind: "text" as const,
                            offset: 0,
                            text: "Ready",
                            toolId: "",
                            noticeId: "",
                          },
                        ]
                      : [],
                },
              ]),
        ],
        pending:
          phase === "queued"
            ? [
                {
                  executionId: retryId,
                  text: "Try again",
                  attachments: [],
                  files: [],
                  mode: "queued",
                },
              ]
            : [],
      }),
      phase === "queued" ? 2 : phase === "running" ? 3 : 4,
      () => 1000,
    ),
  })
}
let causalRevision = 10
function publishRefusal(executionId: string) {
  const transcript = transcriptFrom(
    view("b", {
      runtime: {
        agent: provider,
        provider,
        model: "unknown",
        modelName: "Unknown",
        workspace: "/tmp",
        contextWindowTokens: 200000,
        reasoning: true,
      },
      messages: [
        {
          executionId,
          userText: "Later wire input",
          attachments: [],
          files: [],
          status: "failed",
          authenticationRequired: true,
          parts: [],
        },
      ],
    }),
    ++causalRevision,
    () => 1000,
  )
  source.transcripts.set("b", transcript)
  source.emit({ kind: "transcript", transcript })
}
async function failLocal(text: string) {
  source.hold("send")
  const sent = store.dispatch(sendMessage({ initiator: "person", sessionId: "b", text }))
  // The real command publishes the local input before it awaits the source.
  source.refuse("send", "unavailable")
  await source.release("send")
  await sent
}
let resolve: (() => void) | undefined
const fixture = {
  calls: [] as string[],
  fail: false,
  release() {
    resolve?.()
    resolve = undefined
  },
  sendRetry() {
    source.hold("send")
    void store.dispatch(
      sendMessage({ initiator: "person", sessionId: "b", text: "Try again" }),
    )
  },
  failRetry() {
    source.refuse("send", "unavailable")
    void source.release("send")
  },
  acceptRetry() {
    retryId = store.getState().workspace.outbox.b?.at(-1)?.id
    publishRetry("queued")
  },
  runRetry() {
    publishRetry("running")
  },
  finishRetry() {
    publishRetry("completed")
  },
  outboxCount() {
    return store.getState().workspace.outbox.b?.length ?? 0
  },
  async olderFailure() {
    const transcript = { ...emptyTranscript("b"), revision: ++causalRevision }
    source.transcripts.set("b", transcript)
    source.emit({ kind: "transcript", transcript })
    await failLocal("Older local failure")
    publishRefusal("B")
  },
  async newerFailure() {
    await failLocal("Newer local input")
  },
  repeatRefusal() {
    publishRefusal("B")
  },
  newRefusal() {
    publishRefusal("D")
  },
  recover() {
    source.emit({
      kind: "transcript",
      transcript: { ...emptyTranscript("b"), revision: ++causalRevision },
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
      <Transcript sessionId="b" scrollRef={createRef()} onHeadingVisible={() => {}} />
    </ClockProvider>
  </Provider>,
)
