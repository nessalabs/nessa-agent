// @vitest-environment jsdom
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { expect, it, vi } from "vitest"
import { createDependencies } from "../../composition/dependencies"
import { makeStore } from "../../store"
import { sessionReady } from "../../session/testing"
import { restoreConversations } from "../adapters/store/slice"
import { scenarioEffects } from "../adapters/scenario/effects"
import { useConversation } from "./use-conversation"

const A = "00000000-0000-4000-8000-000000000001"
const B = "00000000-0000-4000-8000-000000000002"

function Followed() {
  const { active } = useConversation()
  return React.createElement("span", null, active.serverConversationId)
}

it("LT6: restoring a ready tab stops its old follow and starts the replacement's follow", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const stops: ReturnType<typeof vi.fn>[] = []
  const followed: string[] = []
  const store = makeStore(
    createDependencies({
      conversation: {
        ...scenarioEffects("echo"),
        follow: (id) => {
          followed.push(id)
          const stop = vi.fn()
          stops.push(stop)
          return stop
        },
      },
    }),
  )
  store.dispatch(
    sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
  )
  store.dispatch(restoreConversations({ tabs: [{ conversationId: A }] }))
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  try {
    await React.act(async () => {
      root.render(
        React.createElement(Provider, { store, children: React.createElement(Followed) }),
      )
    })
    expect(followed).toEqual([A])
    await React.act(async () => {
      store.dispatch(restoreConversations({ tabs: [{ conversationId: B }] }))
    })
    expect(followed).toEqual([A, B])
    expect(stops[0]).toHaveBeenCalledTimes(1)
    expect(stops[1]).not.toHaveBeenCalled()
    expect(host.textContent).toBe(B)
  } finally {
    await React.act(async () => root.unmount())
    host.remove()
  }
  expect(stops[1]).toHaveBeenCalledTimes(1)
})
