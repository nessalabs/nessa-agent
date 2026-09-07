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
          version: 1,
          gatewayId: "gateway",
          principalId: "panel",
          organizationId: "org",
          membershipId: "panel-member",
          credentialId: "panel-token",
          audienceId: "gateway",
          expiresAt: null,
          grants: [],
          methods: ["server.health"],
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
