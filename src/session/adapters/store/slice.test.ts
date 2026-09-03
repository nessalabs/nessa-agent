import { describe, expect, it } from "vitest"

import { makeStore } from "../../../store"
import {
  sessionConnecting,
  sessionDisconnected,
  sessionError,
  sessionReady,
} from "./slice"

describe("session store", () => {
  it("starts idle", () => {
    const session = makeStore().getState().session
    expect(session.phase).toBe("idle")
    expect(session.hello).toBeNull()
  })

  it("records a ready hello and health payload", () => {
    const store = makeStore()
    store.dispatch(sessionConnecting())
    store.dispatch(
      sessionReady({
        hello: {
          protocol: 1,
          scopes: ["server.read"],
          serverVersion: "0.1.0",
          runtimeStatus: "ready",
          policy: { maxPayloadBytes: 65536 },
        },
        health: { ok: true, runtimeStatus: "ready", uptimeMs: 1 },
      }),
    )
    expect(store.getState().session.phase).toBe("ready")
    expect(store.getState().session.health?.ok).toBe(true)
  })

  it("records errors and disconnects", () => {
    const store = makeStore()
    store.dispatch(sessionError("boom"))
    expect(store.getState().session.phase).toBe("error")
    expect(store.getState().session.detail).toBe("boom")
    store.dispatch(sessionDisconnected())
    expect(store.getState().session.detail).toMatch(/Disconnected/)
  })
})
