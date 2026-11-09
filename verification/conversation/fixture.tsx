/** Browser inputs are published gateway views; authority is tested in the server projection. */
import { configureStore } from "@reduxjs/toolkit"
import { useEffect, useState } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { makeStore } from "../../src/store"
import { createDependencies } from "../../src/composition/dependencies"
import { scenarioEffects } from "../../src/conversation/adapters/scenario/effects"
import {
  bindConversation,
  openConversation,
  setActive,
  closeTab,
  followConversation,
  openListed,
} from "../../src/conversation/adapters/store/slice"
import {
  useConversationDispatch,
  useConversationSelector,
} from "../../src/conversation/adapters/store/hooks"
import { createConversationApi } from "../../packages/nessa-client/src/presentation/conversation-api"
import { conversationTabSnapshot } from "../../src/conversation/application/saved-tabs"
import { followByReading } from "../../src/conversation/testing"
import { applyView } from "../../src/conversation/application/usecases/apply-view"
import {
  conversation,
  type Conversation,
  type ConversationTranscriptState,
} from "../../src/conversation/model"
import { ClockProvider } from "../../src/clock"
import { ConversationControls } from "../../src/conversation/ui/conversation-controls"
import { ConversationQuestions } from "../../src/conversation/ui/conversation-questions"
import { ConversationNotification } from "../../src/conversation/ui/conversation-notification"
import {
  decodePanelView,
  publishedView,
  publishedQuestionView,
  questionView,
  states,
  view,
} from "./fixture-state"
import "../../src/styles.css"

const store = configureStore({ reducer: () => ({}) })
/** Actual read projection, saved selection and close thunk; the remote port records dispatch. */
function HistoryTabConsumer() {
  const [result, setResult] = useState<{
    name: string
    saved: number
    closes: number
    open: boolean
  }>()
  const [active, setActive] = useState<{
    store: ReturnType<typeof makeStore>
    closeCalls: () => number
  }>()
  const show = async (state: ConversationTranscriptState, truncated = false) => {
    let closes = 0
    const serverId = "0b8f1c2e-1111-4a4a-8b8b-000000000001"
    const effects = {
      ...scenarioEffects("echo"),
      follow: followByReading(async (id: string) =>
        decodePanelView(
          {
            ...publishedView(state, false),
            conversationId: id,
            messages: [],
            permissions: [],
            truncated,
          },
          id,
        ),
      ),
      close: async () => {
        closes += 1
      },
    }
    const tabs = makeStore(createDependencies({ conversation: effects }))
    tabs.dispatch(openListed({ serverConversationId: serverId, title: null }))
    await tabs
      .dispatch(followConversation(tabs.getState().conversation.activeId))
      .unwrap()
    setActive({ store: tabs, closeCalls: () => closes })
    setResult({
      name: `${state}:${truncated}`,
      saved: conversationTabSnapshot(tabs.getState().conversation).tabs.length,
      closes,
      open: true,
    })
  }
  const close = async () => {
    if (!active) return
    const closedId = active.store.getState().conversation.activeId
    await active.store.dispatch(closeTab(closedId)).unwrap()
    setResult(
      (previous) =>
        previous && {
          ...previous,
          closes: active.closeCalls(),
          open: active.store
            .getState()
            .conversation.conversations.some((item) => item.id === closedId),
        },
    )
  }
  return (
    <section
      data-history-tab-consumer
      data-case={result?.name}
      data-saved-count={result?.saved}
      data-close-calls={result?.closes}
      data-original-tab-present={result?.open}
    >
      <nav aria-label="Saved history cases">
        {states.map((state) => (
          <button key={state} onClick={() => void show(state)}>
            tabs:{state}
          </button>
        ))}
        <button onClick={() => void show("complete_empty", true)}>
          tabs:complete_empty:truncated
        </button>
      </nav>
      <button disabled={!active} onClick={() => void close()}>
        Close history tab
      </button>
      <p>
        Saved references: {result?.saved ?? 0}; gateway close calls: {result?.closes ?? 0}
      </p>
    </section>
  )
}

const questionConversations = [
  "00000000-0000-4000-8000-000000000001",
  "00000000-0000-4000-8000-000000000002",
] as const

/** Real panel controls and client serialization; this recorder is not an authenticated gateway. */
function QuestionTargetConsumer() {
  const [requests, setRequests] = useState<unknown[]>([])
  const [tabs] = useState(() => {
    let action = 0
    const client = createConversationApi(
      {
        request: async (_method, params) => {
          setRequests((previous) => [...previous, params])
          return { requestId: (params as { requestId: string }).requestId, applied: true }
        },
      },
      () => `question-action-${++action}`,
    )
    const effects = {
      ...scenarioEffects("echo"),
      create: async (conversationId: string) => ({ conversationId }),
      follow: followByReading(async (id: string) =>
        decodePanelView({ ...publishedQuestionView(false), conversationId: id }, id),
      ),
      answerQuestion: async (...args: Parameters<typeof client.answerQuestion>) => {
        await client.answerQuestion(...args)
      },
    }
    const result = makeStore(createDependencies({ conversation: effects }))
    result.dispatch(bindConversation({ id: "c0", serverId: questionConversations[0] }))
    result.dispatch(openConversation())
    result.dispatch(bindConversation({ id: "c1", serverId: questionConversations[1] }))
    result.dispatch(setActive("c0"))
    return result
  })
  useEffect(() => {
    void tabs.dispatch(followConversation("c0"))
    void tabs.dispatch(followConversation("c1"))
  }, [tabs])
  return (
    <section data-question-target-consumer data-requests={JSON.stringify(requests)}>
      <Provider store={tabs}>
        <QuestionTargetTabs />
      </Provider>
    </section>
  )
}
function QuestionTargetTabs() {
  const dispatch = useConversationDispatch()
  const active = useConversationSelector((state) =>
    state.conversation.conversations.find(
      (item) => item.id === state.conversation.activeId,
    ),
  )
  return (
    <>
      <nav aria-label="Question conversations">
        <button onClick={() => dispatch(setActive("c0"))}>
          First question conversation
        </button>
        <button onClick={() => dispatch(setActive("c1"))}>
          Second question conversation
        </button>
      </nav>
      <div data-question-target-active={active?.serverConversationId}>
        {active && <ConversationQuestions conversation={active} gatewayAvailable />}
      </div>
    </>
  )
}

function Fixture() {
  const [current, setCurrent] = useState<Conversation>(() => ({
    ...conversation("committed-conversation"),
    serverConversationId: "committed-conversation",
  }))
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
      <HistoryTabConsumer />
      <QuestionTargetConsumer />
      <section data-committed-controls>
        <ConversationControls conversation={current} gatewayAvailable />
      </section>
    </main>
  )
}
createRoot(document.getElementById("root")!).render(
  <Provider store={store}>
    {/* A fixture is its own composition root, so it chooses the clock. */}
    <ClockProvider now={() => Date.now()}>
      <Fixture />
    </ClockProvider>
  </Provider>,
)
