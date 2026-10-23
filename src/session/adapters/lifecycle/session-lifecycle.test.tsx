// @vitest-environment jsdom
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { configureStore } from "@reduxjs/toolkit"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { NessaConnectionClosedError } from "@nessa/client"
import { createSessionHandle } from "../client/handle"
import type { EstablishedDevSession } from "../client/dev-session"
import { retrySession, sessionReducer } from "../store/slice"
import { SessionLifecycle } from "./session-lifecycle"

type Client = EstablishedDevSession["client"]
function connection() {
  let state: Client["connectionState"] = { status: "connected" }
  const closed = new Set<(error: Error) => void>()
  const states = new Set<(state: Client["connectionState"]) => void>()
  const close = vi.fn()
  const client = {
    get connectionState() {
      return state
    },
    productSession: { gatewayId: "fixture-gateway" },
    close,
    onClose: (callback: (error: Error) => void) => {
      closed.add(callback)
      return () => {
        closed.delete(callback)
      }
    },
    onConnectionStateChange: (callback: (state: Client["connectionState"]) => void) => {
      states.add(callback)
      return () => {
        states.delete(callback)
      }
    },
  } as unknown as Client
  const established = {
    client,
    hello: client.productSession,
    health: { ok: true, runtimeStatus: "ready", uptimeMs: 1 },
  } as EstablishedDevSession
  return {
    established,
    close,
    listeners: () => closed.size + states.size,
    change(next: Client["connectionState"]) {
      state = next
      for (const callback of [...states]) callback(next)
    },
    end(error: Error) {
      state = { status: "closed", error }
      for (const callback of [...closed]) callback(error)
    },
  }
}
let root: Root
beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  root = createRoot(document.createElement("div"))
})
afterEach(async () => {
  await React.act(async () => root.unmount())
})
function scope(connectSession: () => Promise<EstablishedDevSession>) {
  const store = configureStore({ reducer: { session: sessionReducer } })
  const session = createSessionHandle()
  const dependencies = { session, connectSession }
  const mount = (strict = false) =>
    React.act(async () => {
      const lifecycle = (
        <Provider store={store}>
          <SessionLifecycle dependencies={dependencies} />
        </Provider>
      )
      root.render(strict ? <React.StrictMode>{lifecycle}</React.StrictMode> : lifecycle)
    })
  return { store, session, mount }
}

it("keeps the Redux projection and command handle in agreement through reconnect and unmount", async () => {
  const active = connection(),
    connect = vi.fn(async () => active.established)
  const owner = scope(connect)
  await owner.mount()
  expect(owner.store.getState().session).toMatchObject({
    phase: "ready",
    hello: { gatewayId: "fixture-gateway" },
  })
  expect(owner.session.get()).toBe(active.established.client)
  await React.act(async () =>
    active.change({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, "interrupted"),
    }),
  )
  expect(owner.store.getState().session).toMatchObject({
    phase: "reconnecting",
    hello: null,
    health: null,
  })
  expect(owner.session.get()).toBeNull()
  await React.act(async () => active.change({ status: "connected" }))
  expect(owner.store.getState().session.phase).toBe("ready")
  expect(owner.session.get()).toBe(active.established.client)
  expect(connect).toHaveBeenCalledOnce()
  await React.act(async () => root.render(null))
  expect(owner.session.get()).toBeNull()
  expect(active.listeners()).toBe(0)
  expect(active.close).toHaveBeenCalledOnce()
})

it("explicit Retry retires the failed effect and creates a fresh connected lifetime", async () => {
  const first = connection(),
    next = connection()
  const connect = vi
    .fn()
    .mockResolvedValueOnce(first.established)
    .mockResolvedValue(next.established)
  const owner = scope(connect)
  await owner.mount()
  await React.act(async () => first.end(new NessaConnectionClosedError(4003, "expired")))
  expect(owner.store.getState().session).toMatchObject({
    phase: "error",
    hello: null,
    health: null,
  })
  expect(owner.session.get()).toBeNull()
  expect(first.listeners()).toBe(0)
  expect(connect).toHaveBeenCalledOnce()
  await React.act(async () => {
    owner.store.dispatch(retrySession())
  })
  expect(connect).toHaveBeenCalledTimes(2)
  expect(owner.store.getState().session).toMatchObject({
    phase: "ready",
    retryRequest: 1,
  })
  expect(owner.session.get()).toBe(next.established.client)
})

it("Strict Mode closes the obsolete pending result and publishes only the replacement", async () => {
  let resolve!: (value: EstablishedDevSession) => void
  const stale = connection(),
    current = connection()
  const pending = new Promise<EstablishedDevSession>((done) => {
    resolve = done
  })
  const connect = vi
    .fn()
    .mockReturnValueOnce(pending)
    .mockResolvedValue(current.established)
  const owner = scope(connect)
  await owner.mount(true)
  expect(connect).toHaveBeenCalledTimes(2)
  expect(owner.session.get()).toBe(current.established.client)
  await React.act(async () => {
    resolve(stale.established)
  })
  expect(stale.close).toHaveBeenCalledOnce()
  expect(stale.listeners()).toBe(0)
  expect(current.close).not.toHaveBeenCalled()
  expect(current.listeners()).toBe(2)
  expect(owner.session.get()).toBe(current.established.client)
  expect(owner.store.getState().session.phase).toBe("ready")
})
