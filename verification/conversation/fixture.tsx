/** Browser inputs are published gateway views; authority is tested in the server projection. */
import { configureStore } from "@reduxjs/toolkit"
import { useState } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { conversationView as decodeView } from "../../packages/nessa-client/src/protocol/conversation-validate"
import { applyView } from "../../src/conversation/application/usecases/apply-view"
import type { ConversationView } from "../../src/conversation/application/view"
import {
  conversation,
  type ConversationTranscriptState,
} from "../../src/conversation/model"
import { ConversationControls } from "../../src/conversation/ui/conversation-controls"
import { ConversationQuestions } from "../../src/conversation/ui/conversation-questions"
import { ConversationNotification } from "../../src/conversation/ui/conversation-notification"
import "../../src/styles.css"

const store = configureStore({ reducer: () => ({}) })
const states: ConversationTranscriptState[] = [
  "not_loaded",
  "partial",
  "stale",
  "unknown",
  "complete_empty",
  "complete",
]
function view(
  state: ConversationTranscriptState,
  liveAuthority: boolean,
): ConversationView {
  const actionable = state === "complete" && liveAuthority
  return decodeView(
    {
      conversationId: "committed-conversation",
      revision: `${state}:${liveAuthority}`,
      title: "Committed transcript",
      approvalMode: "ask",
      approvalModes: [
        { id: "ask", name: "Provider asks", description: "Provider permission review." },
      ],
      transcriptState: state,
      queueComplete: state === "complete" || state === "complete_empty",
      truncated: false,
      messages:
        state === "complete"
          ? [
              {
                executionId: "exact-live-execution",
                userText: "Read committed source",
                attachments: [],
                files: [],
                status: liveAuthority ? "running" : "unresolved",
                parts: [],
              },
            ]
          : [],
      pending: [],
      questions: [],
      tools: [],
      permissions: actionable
        ? [
            {
              executionId: "exact-live-execution",
              permissionId: "committed-review",
              toolId: "tool",
              toolName: "Read",
              title: "Read committed source",
              argumentsJson: '{"path":"src/main.rs"}',
              options: [{ id: "allow", label: "Allow once" }],
            },
          ]
        : [],
      capabilities: {
        queue: actionable,
        steer: actionable,
        resume: false,
        permissions: actionable,
        imageInput: false,
        agentFeatures: {
          permissionDenial: "unknown",
          nativeHookSuppression: "unknown",
          compactionReporting: "unsupported_not_implemented",
          modelSwitchReporting: "unsupported_not_implemented",
          permissionDeferral: "unsupported_not_implemented",
          elicitationForwarding: "unknown",
          preToolPolicy: "unsupported_not_implemented",
          policyEndTurn: "unsupported_not_implemented",
          policyCloseSession: "unsupported_not_implemented",
          incomingElicitation: "unknown",
        },
      },
      lifecycle: { phase: liveAuthority ? "attached" : "absent" },
    },
    "committed-conversation",
  )
}
function questionView(limited: boolean): ConversationView {
  return decodeView(
    {
      ...view("complete", true),
      revision: `questions:${limited}`,
      truncated: limited,
      interactionViewError: limited
        ? "Some pending interactions exceed this display limit. Use Stop to cancel them."
        : undefined,
      permissions: [],
      questions: [
        {
          executionId: "exact-live-execution",
          questionId: "retained-question",
          message: "Choose the environments",
          questions: [0, 1, 2].map((index) => ({
            key: `environment_${index}`,
            prompt: `Environment ${index}?`,
            multiSelect: false,
            freeText: false,
            required: false,
            options: [{ value: "staging", label: "Staging" }],
          })),
        },
      ],
    },
    "committed-conversation",
  )
}
function Fixture() {
  const [current, setCurrent] = useState(() => conversation("committed-conversation"))
  const [caseName, setCaseName] = useState("initial")
  const show = (state: ConversationTranscriptState, authority: boolean) => {
    setCurrent((previous) => applyView(previous, view(state, authority)))
    setCaseName(`${state}:${authority}`)
  }
  const showQuestions = (limited: boolean) => {
    setCurrent((previous) => applyView(previous, questionView(limited)))
    setCaseName(`questions:${limited}`)
  }
  return (
    <main
      data-committed-fixture
      data-case={caseName}
      style={{ padding: 24, maxWidth: 600 }}
    >
      <nav
        aria-label="Published view cases"
        style={{ display: "flex", flexWrap: "wrap", gap: 8, marginBottom: 24 }}
      >
        {states.map((state) => (
          <button key={state} onClick={() => show(state, true)}>
            {state}
          </button>
        ))}
        <button onClick={() => show("complete", false)}>
          complete without live attachment
        </button>
        <button onClick={() => showQuestions(false)}>questions</button>
        <button onClick={() => showQuestions(true)}>questions with display limit</button>
      </nav>
      <section data-committed-limit-notice>
        <ConversationNotification
          conversation={current}
          gatewayAvailable
          connection={{ phase: "ready", detail: "", retry: () => {} }}
        />
      </section>
      <section data-committed-questions>
        <ConversationQuestions conversation={current} gatewayAvailable />
      </section>
      <section data-committed-controls>
        <ConversationControls conversation={current} gatewayAvailable />
      </section>
    </main>
  )
}
createRoot(document.getElementById("root")!).render(
  <Provider store={store}>
    <Fixture />
  </Provider>,
)
