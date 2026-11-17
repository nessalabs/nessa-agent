import { expect, it } from "vitest"
import {
  reduceGatewayInvitation,
  type GatewayInvitationStatus,
} from "./gateway-invitation"

const claimed: GatewayInvitationStatus = {
  invitationId: Array(16).fill(1),
  consentId: Array(16).fill(2),
  generation: 1,
  class: "peer-gateway-conversation-read",
  samePerson: true,
  grant: {
    action: "conversation.read",
    resource: { organizationId: "org", id: "gateway" },
  },
  createdAtMs: 1,
  expiresAtMs: 600001,
  phase: "claimed",
  cleanupPending: false,
  claimedDeviceKey: Array(32).fill(7),
}

it("rejects late fingerprint evidence for another invitation or claimed key", () => {
  const other = { ...claimed, invitationId: Array(16).fill(3) }
  const current = reduceGatewayInvitation(undefined, { type: "resume", status: other })
  expect(
    reduceGatewayInvitation(current, {
      type: "fingerprint",
      invitationId: claimed.invitationId,
      key: claimed.claimedDeviceKey!,
      value: "old invitation",
    }),
  ).toBe(current)
  expect(
    reduceGatewayInvitation(current, {
      type: "fingerprint",
      invitationId: other.invitationId,
      key: Array(32).fill(8),
      value: "other key",
    }),
  ).toBe(current)
  expect(
    reduceGatewayInvitation(current, {
      type: "fingerprint",
      invitationId: other.invitationId,
      key: other.claimedDeviceKey!,
      value: "matching",
    })?.fingerprint,
  ).toBe("matching")
})

it("resuming replaces old details and a different invitation read cannot inherit them", () => {
  const made = reduceGatewayInvitation(undefined, {
    type: "create",
    reply: { status: claimed, code: "ABCD-EFGH" },
  })
  const marked = reduceGatewayInvitation(made, {
    type: "fingerprint",
    invitationId: claimed.invitationId,
    key: claimed.claimedDeviceKey!,
    value: "old fingerprint",
  })
  const stopped = reduceGatewayInvitation(marked, {
    type: "approve",
    reply: { status: { ...claimed, phase: "approved" }, activationStopped: "permanent" },
  })
  expect(stopped).toMatchObject({
    code: "ABCD-EFGH",
    activationStopped: "permanent",
    fingerprint: "old fingerprint",
  })
  expect(reduceGatewayInvitation(stopped, { type: "resume", status: claimed })).toEqual({
    status: claimed,
  })
  const other = { ...claimed, invitationId: Array(16).fill(3) }
  const read = reduceGatewayInvitation(stopped, { type: "read", status: other })
  expect(read?.status).toBe(other)
  expect(read?.code).toBeUndefined()
  expect(read?.activationStopped).toBeUndefined()
  expect(read?.fingerprint).toBeUndefined()
})
