import { describe, expect, it, vi } from "vitest"

import { NessaPairingError, PairingRefusalCode } from "../application/pairing-error.js"
import { NessaRpcError } from "../application/rpc-error.js"
import {
  PairingActivationStop,
  PairingEnrollee,
  PairingOwnerPhase,
} from "../generated/product.js"
import { createPairingApi } from "./pairing-api.js"

const invitationId = Array.from({ length: 16 }, (_, index) => index)
const deviceKey = Array.from({ length: 32 }, (_, index) => index + 1)
const consentId = Array.from({ length: 16 }, (_, index) => 200 - index)

const status = {
  invitationId,
  consentId,
  generation: 1,
  class: "gateway-conversation-read",
  grant: {
    action: "conversation.read",
    resource: { organizationId: "personal", id: "gateway" },
  },
  createdAtMs: 1_000,
  expiresAtMs: 61_000,
  phase: PairingOwnerPhase.Available,
  cleanupPending: false,
}

function session(answer: (method: string, params: unknown) => unknown) {
  const calls: { method: string; params: unknown }[] = []
  const request = vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params })
    return answer(method, params)
  })
  return { api: createPairingApi({ request }), calls }
}

describe("client.pairing", () => {
  it("creates with an empty object and returns the code once", async () => {
    const { api, calls } = session(() => ({ code: "ABCD-2345", status }))
    const created = await api.create()
    expect(calls).toEqual([{ method: "pairing.create", params: {} }])
    expect(created.code).toBe("ABCD-2345")
    expect(created.status.phase).toBe("available")
  })

  it("names a peer gateway as the enrollee when asked", async () => {
    const { api, calls } = session(() => ({
      code: "ABCD-2345",
      status: { ...status, class: "peer-gateway-conversation-read" },
    }))
    const created = await api.create({ enrollee: PairingEnrollee.Gateway })
    expect(calls).toEqual([{ method: "pairing.create", params: { enrollee: "gateway" } }])
    expect(created.status.class).toBe("peer-gateway-conversation-read")
  })

  it("lists pending enrollments and reads one by its invitation", async () => {
    const claimed = {
      ...status,
      phase: PairingOwnerPhase.Claimed,
      claimedDeviceKey: deviceKey,
    }
    const { api, calls } = session((method) =>
      method === "pairing.pending" ? { items: [claimed] } : claimed,
    )
    expect((await api.pending()).items).toHaveLength(1)
    const read = await api.status(invitationId)
    expect(read.claimedDeviceKey).toEqual(deviceKey)
    expect(calls[1]).toEqual({
      method: "pairing.status",
      params: { invitationId },
    })
  })

  it("approves the exact key and keeps a typed activation stop", async () => {
    const { api, calls } = session(() => ({
      status: { ...status, phase: PairingOwnerPhase.Staging },
      activationStopped: PairingActivationStop.Retryable,
    }))
    const approved = await api.approve(invitationId, deviceKey)
    expect(calls[0]?.params).toEqual({ invitationId, deviceKey })
    expect(approved.activationStopped).toBe("retryable")
    expect(approved.status.phase).toBe("staging")
  })

  it("refuses a code this build knows, and leaves an unknown code untyped", async () => {
    const refused = session(() => {
      throw new NessaRpcError(PairingRefusalCode.PairingNotConfigured, "no")
    })
    const error = await refused.api.pending().then(
      () => undefined,
      (cause: unknown) => cause,
    )
    expect(error).toBeInstanceOf(NessaPairingError)
    expect((error as NessaPairingError).refusal).toBe("pairing_not_configured")

    const unknown = session(() => {
      throw new NessaRpcError("made_up", "no")
    })
    const other = await unknown.api.create().then(
      () => undefined,
      (cause: unknown) => cause,
    )
    expect((other as NessaPairingError).refusal).toBeUndefined()
  })

  it("does not believe a code of the wrong length", async () => {
    const { api } = session(() => ({ code: "ABCD", status }))
    const error = await api.create().then(
      () => undefined,
      (cause: unknown) => cause,
    )
    expect(error).toBeInstanceOf(NessaPairingError)
    expect((error as NessaPairingError).refusal).toBeUndefined()
  })
})
