/**
 * Which of the gateway's credentials is a linked device: the one rule, read
 * by Settings › Linked devices (to list and revoke them) and by the share
 * dialog (to offer them as share targets). A linked device is a non-revoked
 * credential that is not this session's and whose only grant is
 * `conversation.read`. The wire's credential list has no device marker, so a
 * credential issued by hand with only that grant looks the same. Whether a
 * share to it is admitted stays the gateway's to say
 * (`share_target_not_paired`); this only decides what is offered.
 */
import type { CredentialMetadata } from "@nessa/client"
import type { LinkedDevice } from "../model/linked-device"

/** `credential` as a linked device, or nothing. The session's own is never one. */
export function linkedDeviceOf(
  credential: CredentialMetadata,
  sessionId: string,
): LinkedDevice | undefined {
  if (credential.revokedAt !== null || credential.id === sessionId) return undefined
  if (
    credential.grants.length !== 1 ||
    credential.grants[0]?.action !== "conversation.read"
  )
    return undefined
  return { credentialId: credential.id, issuedAt: credential.issuedAt }
}
