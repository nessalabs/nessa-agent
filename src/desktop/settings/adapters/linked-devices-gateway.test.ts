import {
  NessaMutationError,
  NessaPairingError,
  NessaRpcError,
  PairingRefusalCode,
  type ConnectionState,
} from "@nessa/client"
import { describe, expect, it, vi } from "vitest"
import {
  failureOf,
  linkedDevicesGateway,
  type LinkedDevicesClient,
} from "./linked-devices-gateway"

const invitationId = Array.from({ length: 16 }, (_, index) => index)
const deviceKey = Array.from({ length: 32 }, () => 1)
const invitation = invitationId.map((byte) => byte.toString(16).padStart(2, "0")).join("")

const status = {
  invitationId,
  consentId: Array.from({ length: 16 }, () => 2),
  generation: 1,
  class: "gateway-conversation-read",
  grant: {
    action: "conversation.read",
    resource: { organizationId: "personal", id: "gateway" },
  },
  createdAtMs: 1_000,
  expiresAtMs: 61_000,
  phase: "available" as const,
  cleanupPending: false,
}

function credential(id: string, actions: string[], revokedAt: number | null = null) {
  return {
    id,
    principalId: "owner",
    organizationId: "personal",
    audienceId: "gateway",
    issuedAt: 1_700_000_000,
    expiresAt: null,
    revokedAt,
    grants: actions.map((action) => ({
      action,
      resource: { organizationId: "personal", id: "gateway" },
    })),
  }
}

function client(partial: Partial<LinkedDevicesClient> = {}): LinkedDevicesClient {
  return {
    pairing: {
      create: vi.fn(),
      pending: vi.fn(async () => ({ items: [] })),
      status: vi.fn(),
      approve: vi.fn(),
      deny: vi.fn(),
      cancel: vi.fn(),
    },
    credentials: {
      issue: vi.fn(),
      list: vi.fn(async () => ({ credentials: [] })),
      revoke: vi.fn(),
    },
    auth: {
      session: vi.fn(async () => ({
        version: 1 as const,
        gatewayId: "gateway",
        principalId: "owner",
        organizationId: "personal",
        membershipId: "membership",
        credentialId: "session-credential",
        audienceId: "gateway",
        expiresAt: null,
        grants: [],
        methods: [],
      })),
    },
    connectionState: { status: "connected" } as ConnectionState,
    onConnectionStateChange: () => () => {},
    ...partial,
  }
}

describe("failureOf", () => {
  it("L18: maps every pairing refusal, and an unknown code stays unanswered", () => {
    for (const code of Object.values(PairingRefusalCode)) {
      const failure = failureOf(
        new NessaPairingError("pairing.pending", new NessaRpcError(code, "refused")),
      )
      expect(failure.kind).not.toBe("unanswered")
      expect(failure.kind).not.toContain("_")
    }
    expect(
      failureOf(
        new NessaPairingError("pairing.pending", new NessaRpcError("made_up", "no")),
      ),
    ).toEqual({ kind: "unanswered" })
    expect(failureOf(new Error("lost"))).toEqual({ kind: "unanswered" })
  })

  it("maps a missing credential, and an unknown credential code stays unanswered", () => {
    expect(
      failureOf(
        new NessaMutationError("req", new NessaRpcError("credential_not_found", "gone")),
      ),
    ).toEqual({ kind: "credentialMissing" })
    expect(
      failureOf(new NessaMutationError("req", new NessaRpcError("made_up", "no"))),
    ).toEqual({ kind: "unanswered" })
  })
})

describe("linkedDevicesGateway", () => {
  const gatewayFor = (current: LinkedDevicesClient) =>
    linkedDevicesGateway({
      connected: async () => current,
      after: () => () => {},
      pollMs: 0,
    })

  it("L2 and L3: linking off and a refused owner are successful reads", async () => {
    const off = client()
    vi.mocked(off.pairing.pending).mockRejectedValue(
      new NessaPairingError(
        "pairing.pending",
        new NessaRpcError("pairing_not_configured", "off"),
      ),
    )
    await expect(gatewayFor(off).read()).resolves.toEqual({
      ok: true,
      value: { kind: "off" },
    })
    expect(off.credentials.list).not.toHaveBeenCalled()

    const refused = client()
    vi.mocked(refused.pairing.pending).mockRejectedValue(
      new NessaPairingError("pairing.pending", new NessaRpcError("forbidden", "no")),
    )
    await expect(gatewayFor(refused).read()).resolves.toEqual({
      ok: true,
      value: { kind: "refused" },
    })
  })

  it("L20: lists only a non-revoked conversation.read credential that is not this session", async () => {
    const current = client()
    vi.mocked(current.credentials.list).mockResolvedValue({
      credentials: [
        credential("session-credential", ["conversation.read"]),
        credential("owner", ["conversation.read", "credential.manage"]),
        credential("revoked", ["conversation.read"], 10),
        credential("device-1", ["conversation.read"]),
      ],
    })
    const read = await gatewayFor(current).read()
    expect(read.ok && read.value.kind === "on" && read.value.devices).toEqual([
      { credentialId: "device-1", issuedAt: 1_700_000_000 },
    ])
  })

  it("does not list devices when the session cannot be read", async () => {
    const current = client()
    vi.mocked(current.auth.session).mockRejectedValue(new Error("lost"))
    vi.mocked(current.credentials.list).mockResolvedValue({
      credentials: [credential("device-1", ["conversation.read"])],
    })
    await expect(gatewayFor(current).read()).resolves.toEqual({
      ok: false,
      failure: { kind: "unanswered" },
    })
  })

  it("hashes the claimed key before the reducer sees it", async () => {
    const current = client()
    vi.mocked(current.pairing.pending).mockResolvedValue({
      items: [{ ...status, phase: "claimed", claimedDeviceKey: deviceKey }],
    })
    const read = await gatewayFor(current).read()
    expect(
      read.ok && read.value.kind === "on" && read.value.enrollments[0],
    ).toMatchObject({
      invitationId: invitation,
      phase: "claimed",
      fingerprint: "182ff9da701fd144e2fd2cd41da8ddba979eb01b2bf7fcc3376f4b1b2ecee4e7",
    })
  })

  it("L8: a failed cancel does not create", async () => {
    const current = client()
    vi.mocked(current.pairing.cancel).mockRejectedValue(
      new NessaPairingError("pairing.cancel", new NessaRpcError("pairing_busy", "busy")),
    )
    const refreshed = await gatewayFor(current).refresh(invitation)
    expect(refreshed).toEqual({ ok: false, failure: { kind: "busy" } })
    expect(current.pairing.create).not.toHaveBeenCalled()
  })

  it("a create that fails after cancel says the invitation was cancelled", async () => {
    const current = client()
    vi.mocked(current.pairing.cancel).mockResolvedValue({ ...status, phase: "terminal" })
    vi.mocked(current.pairing.create).mockRejectedValue(
      new NessaPairingError(
        "pairing.create",
        new NessaRpcError("pairing_unavailable", "no"),
      ),
    )
    await expect(gatewayFor(current).refresh(invitation)).resolves.toEqual({
      ok: false,
      failure: { kind: "unavailable" },
      cancelled: true,
    })
  })
})
