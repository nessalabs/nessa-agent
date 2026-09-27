import { describe, expect, it } from "vitest"

import { makeStore } from "../../../store"
import {
  sessionReconnecting,
  sessionConnecting,
  sessionError,
  sessionReady,
} from "./slice"
import { canUseGateway } from "../../model"

const readyPayload = {
  hello: {
    version: 1 as const,
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
  health: { ok: true as const, runtimeStatus: "ready" as const, uptimeMs: 1 },
}

describe("session store", () => {
  it("starts idle", () => {
    const session = makeStore().getState().session
    expect(session.phase).toBe("idle")
    expect(session.hello).toBeNull()
  })

  it("records a ready hello and health payload", () => {
    const store = makeStore()
    store.dispatch(sessionConnecting())
    store.dispatch(sessionReady(readyPayload))
    expect(store.getState().session.phase).toBe("ready")
    expect(store.getState().session.health?.ok).toBe(true)
  })

  it("records errors", () => {
    const store = makeStore()
    store.dispatch(sessionError("boom"))
    expect(store.getState().session.phase).toBe("error")
    expect(store.getState().session.detail).toBe("boom")
  })

  it("withdraws gateway capability while a ready session reconnects", () => {
    const store = makeStore()
    store.dispatch(sessionReady(readyPayload))
    expect(canUseGateway(store.getState().session)).toBe(true)
    expect(canUseGateway({ ...store.getState().session, hello: null })).toBe(false)

    store.dispatch(sessionReconnecting())
    expect(canUseGateway(store.getState().session)).toBe(false)
    expect(store.getState().session.hello).toBeNull()

    store.dispatch(sessionReady(readyPayload))
    expect(canUseGateway(store.getState().session)).toBe(true)
  })
})
