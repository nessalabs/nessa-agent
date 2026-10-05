import { NessaPairingError, type PairingMethod } from "../application/pairing-error.js"
import type { RpcRequester } from "../application/session-port.js"
import { ProductMethod } from "../generated/product.js"
import type {
  PairingApproveResult,
  PairingCreateResult,
  PairingOwnerStatus,
  PairingPendingResult,
} from "../generated/product.js"
import {
  pairingApproveResult,
  pairingCreateResult,
  pairingOwnerStatus,
  pairingPendingResult,
} from "../protocol/pairing-validate.js"

/**
 * The owner's pairing methods: create a one-use code, list what is still
 * unfinished, and approve, deny or cancel one invitation.
 *
 * Every enrollment decision is the gateway's, answered as a typed refusal.
 * The client checks only that each answer is one the schema describes. The
 * code from `create` is shown once and is not stored here. A create that was
 * not answered may still have opened an invitation: `pending` shows it,
 * without the code. Nothing is retried for you.
 */
export type PairingApi = {
  /** Open one invitation. The code is in the answer and nowhere else. */
  create(): Promise<PairingCreateResult>
  /** Unfinished enrollments for this owner, including ones still owed cleanup. No code. */
  pending(): Promise<PairingPendingResult>
  /** One enrollment, as it stands now. */
  status(invitationId: number[]): Promise<PairingOwnerStatus>
  /**
   * Approve the exact claimed key. The approval is committed even when
   * activation stops; `activationStopped` says whether approving again can
   * finish it.
   */
  approve(invitationId: number[], deviceKey: number[]): Promise<PairingApproveResult>
  /** Deny a claimed enrollment. */
  deny(invitationId: number[]): Promise<PairingOwnerStatus>
  /** Cancel an invitation the owner opened. */
  cancel(invitationId: number[]): Promise<PairingOwnerStatus>
}

export function createPairingApi(session: RpcRequester): PairingApi {
  async function call<T>(
    method: PairingMethod,
    params: unknown,
    validate: (value: unknown) => T,
  ): Promise<T> {
    try {
      return validate(await session.request(method, params))
    } catch (cause) {
      if (cause instanceof NessaPairingError) throw cause
      throw new NessaPairingError(method, cause)
    }
  }
  const invitation = (invitationId: number[]) => ({ invitationId })
  return {
    create: () => call(ProductMethod.PairingCreate, {}, pairingCreateResult),
    pending: () => call(ProductMethod.PairingPending, {}, pairingPendingResult),
    status: (invitationId) =>
      call(ProductMethod.PairingStatus, invitation(invitationId), pairingOwnerStatus),
    approve: (invitationId, deviceKey) =>
      call(
        ProductMethod.PairingApprove,
        { invitationId, deviceKey },
        pairingApproveResult,
      ),
    deny: (invitationId) =>
      call(ProductMethod.PairingDeny, invitation(invitationId), pairingOwnerStatus),
    cancel: (invitationId) =>
      call(ProductMethod.PairingCancel, invitation(invitationId), pairingOwnerStatus),
  }
}
