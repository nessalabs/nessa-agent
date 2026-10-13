/**
 * The `SharingSource` over the window's gateway client (nessa_ui#128, #129):
 * `client.conversation.shares`, `.share` and `.unshare`, the linked devices
 * in `client.credentials.list()` (the one rule, `../../../adapters/linked-device.ts`,
 * which Settings reads too), and `client.agents.list()`'s `commandHosts`,
 * read into the model's words (`model/sharing.ts`). The bounds a Drive
 * grant is held to are the ones the schema publishes (`toolPolicyBounds`).
 *
 * Who may be granted what is the gateway's to say, by its answers. Each
 * refusal is mapped by total tables — one over the conversation codes, one
 * over the codes the session answers before a conversation command is
 * dispatched — so a code the protocol adds is a type error here rather than
 * a sentence nobody wrote. An answer that never came, a code this build does
 * not know, or a connection that could not be made is `unanswered`.
 *
 * A session's id is its conversation's (`gateway-source.ts`).
 */
import {
  ConversationErrorCode,
  conversationErrorCode,
  NessaConversationControlError,
  NessaConversationMutationError,
  NessaRpcError,
  toolPolicyBounds,
  type AgentsApi,
  type AuthApi,
  type ConversationApi,
  type ConversationShare,
  type ConversationToolPolicy,
  type CredentialApi,
} from "@nessa/client"
import { linkedDeviceOf } from "../../../adapters/linked-device"
import { SharingError, type SharingSource } from "../../application/ports"
import type {
  ShareGrant,
  ShareRequest,
  SharingFailure,
  SharingRead,
  ToolPolicy,
} from "../../model/sharing"

/** What sharing asks of the window's gateway client. */
export interface SharingClient {
  readonly conversation: Pick<ConversationApi, "share" | "unshare" | "shares">
  readonly credentials: Pick<CredentialApi, "list">
  readonly auth: Pick<AuthApi, "session">
  readonly agents: Pick<AgentsApi, "list">
}

/** Every conversation code, as what it means to sharing. */
const conversationFailure: Record<ConversationErrorCode, SharingFailure> = {
  [ConversationErrorCode.ShareTargetNotPaired]: "not-paired",
  [ConversationErrorCode.InvalidRequest]: "invalid",
  [ConversationErrorCode.ConversationDeleted]: "deleted",
  [ConversationErrorCode.ConversationNotFound]: "not-found",
  [ConversationErrorCode.TemporarilyUnavailable]: "busy",
  [ConversationErrorCode.ConversationCapacity]: "busy",
  [ConversationErrorCode.ConversationStorageUnavailable]: "storage",
  [ConversationErrorCode.ConversationStateUnreadable]: "storage",
  [ConversationErrorCode.AuditUnavailable]: "audit",
  [ConversationErrorCode.ConversationsNotConfigured]: "not-configured",
  [ConversationErrorCode.DrivePolicyUnenforceable]: "unenforceable",
  // Codes the share methods do not answer with: said as a refusal this
  // dialog has no words of its own for, never as one it does.
  [ConversationErrorCode.AgentNotConfigured]: "other",
  [ConversationErrorCode.AgentUnsupported]: "other",
  [ConversationErrorCode.SandboxUnavailable]: "other",
  [ConversationErrorCode.EnvironmentNotConfigured]: "other",
  [ConversationErrorCode.EnvironmentUnavailable]: "other",
  [ConversationErrorCode.EnvironmentVersionMismatch]: "other",
  [ConversationErrorCode.EnvironmentPlatformUnsupported]: "other",
  [ConversationErrorCode.EnvironmentInstallFailed]: "other",
  [ConversationErrorCode.LinkedFileUnreachable]: "other",
  [ConversationErrorCode.ModelUnavailable]: "other",
  [ConversationErrorCode.ApprovalModeUnavailable]: "other",
  [ConversationErrorCode.ApprovalModeNotApplied]: "other",
  [ConversationErrorCode.ApprovalModeUncertain]: "other",
  [ConversationErrorCode.ApprovalRequestConflict]: "other",
  [ConversationErrorCode.TurnRunning]: "other",
  [ConversationErrorCode.UnknownMethod]: "other",
  [ConversationErrorCode.ConversationClosed]: "other",
  [ConversationErrorCode.ConversationConfigurationChanged]: "other",
  [ConversationErrorCode.SubmissionConflict]: "other",
  [ConversationErrorCode.SubmissionUnresolved]: "other",
  [ConversationErrorCode.StalePermission]: "other",
  [ConversationErrorCode.AgentStartupDeadline]: "other",
  [ConversationErrorCode.AgentOperationFailed]: "other",
  [ConversationErrorCode.ImageInputUnsupported]: "other",
  [ConversationErrorCode.AttachmentNotFound]: "other",
  [ConversationErrorCode.AttachmentUnavailable]: "other",
  [ConversationErrorCode.AttachmentCapacity]: "other",
  [ConversationErrorCode.AttachmentStorageUnavailable]: "other",
  [ConversationErrorCode.AttachmentCleanupUnavailable]: "other",
  [ConversationErrorCode.ConversationErasureIncomplete]: "other",
  [ConversationErrorCode.McpAppUnknown]: "other",
  [ConversationErrorCode.McpServerMismatch]: "other",
  [ConversationErrorCode.McpToolNotForApp]: "other",
  [ConversationErrorCode.McpSessionUnavailable]: "other",
  [ConversationErrorCode.McpApprovalDenied]: "other",
  [ConversationErrorCode.McpApprovalExpired]: "other",
  [ConversationErrorCode.McpCancelled]: "other",
  [ConversationErrorCode.McpRequestTooLarge]: "other",
  [ConversationErrorCode.McpResultTooLarge]: "other",
  [ConversationErrorCode.McpTimedOut]: "other",
  [ConversationErrorCode.McpRemoteError]: "other",
  [ConversationErrorCode.McpUnauthorized]: "other",
  [ConversationErrorCode.McpUnreachable]: "other",
  [ConversationErrorCode.McpInsufficientScope]: "other",
  [ConversationErrorCode.IdempotencyConflict]: "other",
  [ConversationErrorCode.MessageQuotaExceeded]: "other",
}

/**
 * The codes the session answers with before a share method is dispatched
 * (`credential.manage` refused, the sign-in no longer accepted), and that
 * `credential.list` and `auth.session` answer with, which are not
 * conversation codes. Those that are (`invalid_request`,
 * `temporarily_unavailable`) are the table above's.
 */
const sessionFailure = {
  forbidden: "refused",
  unauthorized: "signed-out",
} as const satisfies Record<string, SharingFailure>

/** A wire code as sharing's failure; one this build does not know is unanswered. */
function failureOfCode(code: string | undefined): SharingFailure {
  if (code === undefined) return "unanswered"
  const known = conversationErrorCode(code)
  if (known !== undefined) return conversationFailure[known]
  if (Object.hasOwn(sessionFailure, code))
    return sessionFailure[code as keyof typeof sessionFailure]
  return "unanswered"
}

/** What a failed call means to the dialog, by the error's type. */
export function failureOf(error: unknown): SharingFailure {
  if (error instanceof SharingError) return error.reason
  if (
    error instanceof NessaConversationMutationError ||
    error instanceof NessaConversationControlError
  ) {
    if (error.code !== undefined) return conversationFailure[error.code]
    return error.cause instanceof NessaRpcError
      ? failureOfCode(error.cause.code)
      : "unanswered"
  }
  if (error instanceof NessaRpcError) return failureOfCode(error.code)
  return "unanswered"
}

/** A listed policy in the model's words; hosts absent are none. */
function policyOf(policy: ConversationToolPolicy): ToolPolicy {
  const commandHosts = policy.commandHosts ?? []
  return policy.preset !== undefined
    ? { kind: "preset", preset: policy.preset, commandHosts }
    : {
        kind: "rules",
        allow: policy.allow ?? [],
        deny: policy.deny ?? [],
        commandHosts,
      }
}

/** A policy as the wire takes it: hosts only when it names one. */
export function wirePolicy(policy: ToolPolicy): ConversationToolPolicy {
  const hosts = policy.commandHosts.length
    ? { commandHosts: [...policy.commandHosts] }
    : {}
  return policy.kind === "preset"
    ? { preset: policy.preset, ...hosts }
    : { allow: [...policy.allow], deny: [...policy.deny], ...hosts }
}

function grantOf(share: ConversationShare): ShareGrant {
  return {
    credentialId: share.credentialId,
    role: share.role,
    ...(share.policy ? { policy: policyOf(share.policy) } : {}),
    policyRevision: share.policyRevision,
    grantedAtMs: share.grantedAtMs,
  }
}

/** The four answers a read is made of, asked together. */
function answersFor(client: SharingClient, sessionId: string) {
  return Promise.all([
    client.conversation.shares(sessionId),
    client.auth.session(),
    client.credentials.list(),
    client.agents.list(),
  ])
}

/** The read, from the gateway's answers. A fault here is this build's, not the gateway's refusal. */
function readOf([shares, session, credentials, catalog]: Awaited<
  ReturnType<typeof answersFor>
>): SharingRead {
  return {
    grants: shares.items.map(grantOf),
    targets: credentials.credentials.flatMap((credential) => {
      const device = linkedDeviceOf(credential, session.credentialId)
      return device ? [device] : []
    }),
    commandHosts: catalog.commandHosts,
    limits: toolPolicyBounds,
  }
}

/**
 * Run `call` — the connection and the gateway's calls only — its refusal as
 * a typed `SharingError`. What the window makes of an answer runs outside
 * it, so a fault of this build reaches `sharingFailure` as one and is logged.
 */
async function typed<T>(call: () => Promise<T>): Promise<T> {
  try {
    return await call()
  } catch (error) {
    throw new SharingError(failureOf(error))
  }
}

export function sharingGateway(options: {
  readonly connected: () => Promise<SharingClient>
}): SharingSource {
  const { connected } = options
  return {
    read: async (sessionId) =>
      readOf(await typed(async () => answersFor(await connected(), sessionId))),
    share: (sessionId, request: ShareRequest, requestId) =>
      typed(async () => {
        const client = await connected()
        await client.conversation.share(
          sessionId,
          request.credentialId,
          request.role,
          request.policy ? wirePolicy(request.policy) : undefined,
          { requestId },
        )
      }),
    unshare: (sessionId, credentialId, requestId) =>
      typed(async () => {
        const client = await connected()
        await client.conversation.unshare(sessionId, credentialId, { requestId })
      }),
  }
}
