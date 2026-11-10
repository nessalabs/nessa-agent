/** Gateway invitation presentation values, not the registry lifecycle.
 * add-gateway.test.tsx and gateway-pairing.test.ts cover replacement, teardown,
 * receipt ordering and claimed-key correlation. */
import type { PairingApi } from "@nessa/client"

export type GatewayInvitationStatus = Awaited<ReturnType<PairingApi["status"]>>
type Approval = Awaited<ReturnType<PairingApi["approve"]>>
export interface GatewayInvitationSelection {
  readonly status: GatewayInvitationStatus
  readonly code?: string
  readonly activationStopped?: Approval["activationStopped"]
  readonly fingerprint?: string
}
export type GatewayInvitationReceipt =
  | { readonly type: "create"; readonly reply: Awaited<ReturnType<PairingApi["create"]>> }
  | { readonly type: "approve"; readonly reply: Approval }
  | { readonly type: "read" | "resume"; readonly status: GatewayInvitationStatus }
type SelectionEvent =
  | GatewayInvitationReceipt
  | { readonly type: "clear" }
  | {
      readonly type: "fingerprint"
      readonly invitationId: readonly number[]
      readonly key: readonly number[]
      readonly value: string
    }

export function gatewayInvitationStatus(
  receipt: GatewayInvitationReceipt,
): GatewayInvitationStatus {
  switch (receipt.type) {
    case "create":
    case "approve":
      return receipt.reply.status
    case "read":
    case "resume":
      return receipt.status
  }
}
function sameBytes(
  a: readonly number[] | undefined,
  b: readonly number[] | undefined,
): boolean {
  return (
    a !== undefined &&
    b !== undefined &&
    a.length === b.length &&
    a.every((value, index) => value === b[index])
  )
}
function selected(details: GatewayInvitationSelection): GatewayInvitationSelection {
  if (details.status.phase === "terminal") return { status: details.status }
  if (details.status.phase === "active") {
    const { code: _consumed, ...receipt } = details
    return receipt
  }
  return details
}

/** Related display facts move together. A read carries them only for the same
 * invitation, and an approval carries the exact result that produced its status. */
export function reduceGatewayInvitation(
  previous: GatewayInvitationSelection | undefined,
  event: SelectionEvent,
): GatewayInvitationSelection | undefined {
  if (event.type === "clear") return undefined
  if (event.type === "fingerprint") {
    if (
      !previous ||
      !sameBytes(previous.status.invitationId, event.invitationId) ||
      !sameBytes(previous.status.claimedDeviceKey, event.key)
    )
      return previous
    return selected({ ...previous, fingerprint: event.value })
  }
  const status = gatewayInvitationStatus(event)
  const related =
    previous && sameBytes(previous.status.invitationId, status.invitationId)
      ? previous
      : undefined
  const fingerprint =
    related && sameBytes(related.status.claimedDeviceKey, status.claimedDeviceKey)
      ? related.fingerprint
      : undefined
  switch (event.type) {
    case "create":
      return selected({ status, code: event.reply.code })
    case "resume":
      return selected({ status })
    case "read":
      return selected({ ...related, status, fingerprint })
    case "approve":
      return selected({
        ...related,
        status,
        fingerprint,
        activationStopped: event.reply.activationStopped,
      })
  }
}

/** The enrollment command shown by Devices. JSON keeps the code as data.
 * Encoding outside printable ASCII (and the outer quote) keeps one single-quoted
 * argument in POSIX shells, including off-schema gateway replies.
 * gateway-pairing.test.ts checks the representation boundary and exact payload.
 */
export function gatewayEnrollmentCommand(port: number, code: string): string {
  const params = JSON.stringify({ address: `GATEWAY_IP:${port}`, code }).replace(
    /[^\x20-\x26\x28-\x7e]/g,
    (unit) => `\\u${unit.charCodeAt(0).toString(16).padStart(4, "0")}`,
  )
  return `nessa call peer.enroll --params '${params}'`
}
