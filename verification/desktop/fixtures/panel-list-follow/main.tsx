/** Production Messages list, application/store and client subscriptions over a real gateway. */
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { NessaClient } from "@nessa/client"
import { createDependencies } from "../../../../src/composition/dependencies"
import { makeStore } from "../../../../src/store"
import { gatewayEffects } from "../../../../src/conversation/adapters/gateway/effects"
import { ConversationList } from "../../../../src/conversation/ui/conversation-list"
import { openListed } from "../../../../src/conversation/adapters/store/slice"
import { sessionReady } from "../../../../src/session/testing"
import type { ConversationListFollower } from "../../../../src/conversation/application/ports"
import "@fontsource-variable/geist"
import "../../../../src/styles.css"

const target = window.__panelListTarget
const client = await NessaClient.connect({
  stage: "ci",
  url: target.endpoint,
  role: "surface",
  surface: { kind: "panel", instance: "panel-list-follow-browser" },
  client: { id: "panel-list-follow-browser", version: "0.1.0", platform: "browser" },
  profile: "product",
  auth: { credential: target.credential },
})
const effects = gatewayEffects(
  () => client,
  (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
)
let oneShotReads = 0
let opens = 0
let stops = 0
const followers = new Map<boolean, ConversationListFollower>()
const store = makeStore(
  createDependencies({
    conversation: {
      ...effects,
      list: (archived) => {
        oneShotReads++
        return effects.list(archived)
      },
      followList(archived, follower) {
        opens++
        followers.set(archived, follower)
        const stop = effects.followList(archived, follower)
        return () => {
          stops++
          if (followers.get(archived) === follower) followers.delete(archived)
          stop()
        }
      },
    },
  }),
)
store.dispatch(
  sessionReady({ hello: client.productSession, health: await client.server.health() }),
)
const root = createRoot(document.getElementById("root")!)
const mount = () =>
  root.render(
    <Provider store={store}>
      <main
        data-panel-list-follow
        data-nessa-root
        style={{
          width: 400,
          height: 530,
          margin: "40px auto",
          padding: 18,
          borderRadius: 24,
          background: "var(--popover)",
          display: "flex",
          flexDirection: "column",
        }}
      >
        <h1>Messages</h1>
        <ConversationList onSelect={() => {}} onNew={() => {}} />
      </main>
    </Provider>,
  )
window.__panelListFollow = {
  snapshot: () => ({
    rows:
      store.getState().conversationHistory.rows?.map((row) => row.conversationId) ?? null,
    archivedIds: store.getState().conversationHistory.archivedIds,
    failure: store.getState().conversationHistory.failure,
    following: store.getState().conversationHistory.following,
    requestId: store.getState().conversationHistory.requestId,
    oneShotReads,
    opens,
    stops,
  }),
  hold: (conversationId) => {
    store.dispatch(
      openListed({
        serverConversationId: conversationId,
        title: "Panel catalogue first",
      }),
    )
  },
  failArchived: () =>
    followers.get(true)?.failed("unavailable", new Error("controlled list port outage")),
  hide: () => root.render(<p>Messages closed</p>),
  show: mount,
  close: () => {
    root.unmount()
    client.close()
  },
}
mount()
declare global {
  interface Window {
    __panelListTarget: { endpoint: string; credential: string }
    __panelListFollow: {
      snapshot(): {
        rows: string[] | null
        archivedIds: string[]
        failure: string | null
        following: boolean
        requestId: string | null
        oneShotReads: number
        opens: number
        stops: number
      }
      hold(conversationId: string): void
      failArchived(): void
      hide(): void
      show(): void
      close(): void
    }
  }
}
