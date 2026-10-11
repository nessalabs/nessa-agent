/** Actual panel effects/store/tray and workspace source over a real scripted gateway. */
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider, useSelector } from "react-redux"
import { NessaClient, type ApprovalMode } from "@nessa/client"
import { createDependencies } from "../../../../src/composition/dependencies"
import { sessionReady } from "../../../../src/session/adapters/store/slice"
import { makeStore, type RootState } from "../../../../src/store"
import { gatewayEffects } from "../../../../src/conversation/adapters/gateway/effects"
import {
  bindConversation,
  controlConversation,
  followConversation,
} from "../../../../src/conversation/testing"
import { App } from "../../../../src/panel/ui/app"
import { createAttachmentResources } from "../../../../src/panel/adapters/attachment-resources"
import { ConversationReadFailedError } from "../../../../src/conversation/testing"
import { sendDraft } from "../../../../src/conversation/adapters/store/slice"
import { gatewaySource } from "../../../../src/desktop/workspace/adapters/gateway/gateway-source"
import "@fontsource-variable/geist"
import "../../../../src/styles.css"

const target = window.__modeTarget
const client = await NessaClient.connect({
  stage: "ci",
  url: target.endpoint,
  role: "surface",
  surface: { kind: "panel", instance: "mode-publication-browser" },
  client: { id: "mode-publication-browser", version: "0.1.0", platform: "browser" },
  profile: "product",
  auth: { credential: target.credential },
})
const attachments = createAttachmentResources()
const effects = gatewayEffects(
  () => client,
  (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
)
let viewUnavailable = false
let dispatched = 0
const store = makeStore(
  createDependencies({
    conversation: {
      ...effects,
      follow: (id, follower) => {
        if (!viewUnavailable) return effects.follow(id, follower)
        queueMicrotask(() =>
          follower.failed("unavailable", new ConversationReadFailedError("unavailable")),
        )
        return () => {}
      },
      create: (...args) => {
        dispatched++
        return effects.create(...args)
      },
      send: (...args) => {
        dispatched++
        return effects.send(...args)
      },
      steer: (...args) => {
        dispatched++
        return effects.steer(...args)
      },
    },
  }),
)
store.dispatch(
  sessionReady({ hello: client.productSession, health: await client.server.health() }),
)
const localId = store.getState().conversation.activeId
store.dispatch(bindConversation({ id: localId, serverId: target.conversationId }))
await store.dispatch(followConversation(localId)).unwrap()
let sourceMode: ApprovalMode | undefined
const sourceClient = await NessaClient.connect({
  stage: "ci",
  url: target.endpoint,
  role: "surface",
  surface: { kind: "desktop", instance: "mode-publication-source" },
  client: { id: "mode-publication-source", version: "0.1.0", platform: "browser" },
  profile: "product",
  auth: { credential: target.credential },
})
const source = gatewaySource({
  apps: {
    observe: (view) => {
      sourceMode = view.approvalMode
    },
    forget: () => {},
  },
  connect: () => Promise.resolve(sourceClient),
  clock: {
    now: () => performance.now(),
    after: (ms, run) => {
      const timer = setTimeout(run, ms)
      return () => clearTimeout(timer)
    },
  },
})
let transcript = await source.transcript(target.conversationId)
source.subscribe((update) => {
  if (update.kind === "transcript") transcript = update.transcript
})
const current = () => store.getState().conversation.conversations[0]!
window.__modePublication = {
  snapshot: () => ({
    panelMode: current().remote?.approvalMode,
    sourceMode,
    panelRevision: current().revision,
    sourceRevision: transcript.revision,
    sourceMessages: transcript.messages.length,
    readError: current().readError,
    draft: current().draft,
    dispatched,
  }),
  async refreshSource() {
    transcript = await source.transcript(target.conversationId)
    return window.__modePublication.snapshot()
  },
  async repeat(mode) {
    await store
      .dispatch(
        controlConversation({ id: localId, control: { kind: "setApprovalMode", mode } }),
      )
      .unwrap()
    return window.__modePublication.refreshSource()
  },
  async failView() {
    viewUnavailable = true
    await store.dispatch(followConversation(localId)).unwrap()
    const draft = current().draft
    const result = await store.dispatch(
      sendDraft({ id: localId, content: draft, connected: true }),
    )
    return { ...window.__modePublication.snapshot(), declined: result.payload }
  },
  async recoverView() {
    viewUnavailable = false
    await store.dispatch(followConversation(localId)).unwrap()
    return window.__modePublication.snapshot()
  },
  close: () => {
    source.dispose()
    client.close()
  },
}
function Panel() {
  const conversation = useSelector(
    (state: RootState) => state.conversation.conversations[0]!,
  )
  const remote = conversation.remote!
  return (
    <main
      data-mode-panel
      data-nessa-root
      style={{
        width: 440,
        transform: "translateZ(0)",
        padding: 12,
        height: 480,
        margin: "80px auto",
        borderRadius: 24,
        background: "var(--popover)",
      }}
    >
      <p>Conversation tool approval</p>
      <p data-mode-value>{remote.approvalMode}</p>
      <App
        attachmentResources={attachments}
        canChoosePaths={false}
        digest={async () => "unused"}
        loadConversationChoices={async () => ({
          chosenAgent: undefined,
          catalog: { agents: [], environments: [] },
        })}
      />
    </main>
  )
}
createRoot(document.getElementById("root")!).render(
  <Provider store={store}>
    <Panel />
  </Provider>,
)
declare global {
  interface Window {
    __modeTarget: { endpoint: string; credential: string; conversationId: string }
    __modePublication: {
      snapshot(): {
        panelMode: ApprovalMode | undefined
        sourceMode: ApprovalMode | undefined
        panelRevision: string | undefined
        sourceRevision: number
        sourceMessages: number
        readError: string | undefined
        draft: ReturnType<typeof current>["draft"]
        dispatched: number
      }
      refreshSource(): Promise<ReturnType<Window["__modePublication"]["snapshot"]>>
      repeat(
        mode: ApprovalMode,
      ): Promise<ReturnType<Window["__modePublication"]["snapshot"]>>
      failView(): Promise<
        ReturnType<Window["__modePublication"]["snapshot"]> & { declined: unknown }
      >
      recoverView(): Promise<ReturnType<Window["__modePublication"]["snapshot"]>>
      close(): void
    }
  }
}
