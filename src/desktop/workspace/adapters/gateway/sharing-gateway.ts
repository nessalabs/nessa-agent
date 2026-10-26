/**
 * The `SharingSource` over the window's gateway client (nessa_ui#128, #129):
 * `client.conversation.shares`, `.share` and `.unshare`, the linked devices
 * in `client.credentials.list()` (the one rule, `../../../adapters/linked-device.ts`,
 * which Settings reads too), and `client.agents.list()`'s `commandHosts`,
 * read into the model's words (`model/sharing.ts`). The bounds a Drive
 * grant is held to are the ones the schema publishes (`toolPolicyBounds`).
 * Grants are read a page at a time (`sharePage`) on one connection until a
 * page names no cursor. Each page names the catalogue revision it was read
 * at. A later page that names another revision is not kept: the grants and
 * the cursor are dropped and the walk starts again from the first page.
 * After two such starts the read is unanswered.
 *
 * Who may be granted what is the gateway's to say, by its answers. Each
 * refusal is mapped by total tables — one over the conversation codes, one
 * over the codes the session answers before a conversation command is
 * dispatched — so a code the protocol adds is a type error here rather than
 * a sentence nobody wrote. An answer that never came, a code this build does
 * not know, an answer the client's own validation refused, or a connection
 * that could not be made is `unanswered`, and is logged.
 *
 * The grants are read apart from the devices and hosts, so a failure in
 * those leaves the grant list and its Revoke (D4).
 *
 * A session's id is its conversation's (`gateway-source.ts`).
 */
import {
  ConversationErrorCode,
  conversationErrorCode,
  NessaConversationControlError,
  NessaConversationMutationError,
  NessaRpcError,
  sharePage,
  toolPolicyBounds,
  type AgentsApi,
  type AuthApi,
  type ConversationApi,
  type ConversationCapability,
  type ConversationShare,
  type ConversationSharesCursor,
  type ConversationToolPolicy,
  type CredentialApi,
} from "@nessa/client"
import { linkedDeviceOf } from "../../../adapters/linked-device"
import { SharingError, type SharingSource } from "../../application/ports"
import type {
  ShareGrant,
  ShareRequest,
  ShareRole,
  SharingChoices,
  SharingFailure,
  ToolPolicy,
} from "../../model/sharing"

/**
 * How many times a walk may start again after a later page names another
 * catalogue revision. The next drift is unanswered.
 */
const shareWalkRestarts = 2

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
  // Cedar denied the capabilities this share would write. Nothing was
  // written. The dialog has no sentence of its own for which set was refused.
  [ConversationErrorCode.CapabilitiesRefused]: "other",
  [ConversationErrorCode.ShareGrantChanged]: "changed",
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
  [ConversationErrorCode.ModelChangeNotApplied]: "other",
  [ConversationErrorCode.ModelChangeUncertain]: "other",
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
  // A Drive turn's grant moved. Sharing does not send or steer, so these
  // are not a share or an unshare.
  // A channel is named only when a conversation is made.
  [ConversationErrorCode.ChannelNotFound]: "other",
  [ConversationErrorCode.Revoked]: "other",
  [ConversationErrorCode.RevokedAfterSave]: "other",
  [ConversationErrorCode.UnavailableAfterSave]: "other",
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

/** The wire set for one of the three choices the dialog offers. */
function capabilitiesOf(role: ShareRole): ConversationCapability[] {
  switch (role) {
    case "read":
      return ["read"]
    case "comment":
      return ["read", "comment"]
    case "drive":
      return ["read", "comment", "drive"]
  }
}

/** The dialog's choice for a wire set. Any other set is not a grant this build reads. */
function roleOf(capabilities: readonly string[]): ShareRole {
  const names = [...capabilities].sort().join("+")
  switch (names) {
    case "read":
      return "read"
    case "comment+read":
      return "comment"
    case "comment+drive+read":
      return "drive"
    default:
      throw new Error("Invalid conversation share capabilities")
  }
}

function grantOf(share: ConversationShare): ShareGrant {
  return {
    credentialId: share.credentialId,
    role: roleOf(share.capabilities),
    ...(share.policy ? { policy: policyOf(share.policy) } : {}),
    policyRevision: share.policyRevision,
    grantedAtMs: share.grantedAtMs,
  }
}

/** The three answers what the form offers is made of, asked together. */
function choicesFor(client: SharingClient) {
  return Promise.all([
    client.auth.session(),
    client.credentials.list(),
    client.agents.list(),
  ])
}

/** What the form offers, from the gateway's answers. A fault here is this build's, not the gateway's refusal. */
function choicesOf([session, credentials, catalog]: Awaited<
  ReturnType<typeof choicesFor>
>): SharingChoices {
  return {
    targets: credentials.credentials.flatMap((credential) => {
      // The owner's phone already reads every chat; a share would add nothing.
      const device = linkedDeviceOf(credential, session.credentialId)
      return device && !device.fullAccess ? [device] : []
    }),
    commandHosts: catalog.commandHosts,
    limits: toolPolicyBounds,
  }
}

/**
 * Run `call` — the connection and the gateway's calls only — its refusal as
 * a typed `SharingError`. What the window makes of an answer runs outside
 * it, so a fault of this build reaches `sharingFailure` as one and is logged.
 * An `unanswered` is logged here, with what made it so: no connection, an
 * answer the client's own validation refused, or a code this build does not
 * know. The dialog says only that no answer came.
 */
async function typed<T>(call: () => Promise<T>): Promise<T> {
  try {
    return await call()
  } catch (error) {
    const reason = failureOf(error)
    if (reason === "unanswered")
      console.warn("Sharing got no answer it could read", error)
    throw new SharingError(reason)
  }
}

export function sharingGateway(options: {
  readonly connected: () => Promise<SharingClient>
}): SharingSource {
  const { connected } = options
  return {
    grants: (sessionId) =>
      typed(async () => {
        const client = await connected()
        let restarts = 0
        for (;;) {
          const grants: ShareGrant[] = []
          const seen = new Set<string>()
          let cursor: ConversationSharesCursor | undefined
          let revision: number | undefined
          for (;;) {
            const page = await client.conversation.shares(sessionId, {
              pageSize: sharePage.pageSize,
              ...(cursor === undefined ? {} : { cursor }),
            })
            // This page was sliced with the cursor of another revision, so
            // it is not part of the list that revision names.
            if (revision !== undefined && page.changeRevision !== revision) {
              restarts += 1
              if (restarts > shareWalkRestarts)
                throw new Error("conversation.shares page did not advance")
              break
            }
            revision = page.changeRevision
            for (const item of page.items) {
              if (seen.has(item.credentialId) || grants.length >= sharePage.grants)
                throw new Error("conversation.shares page did not advance")
              seen.add(item.credentialId)
              grants.push(grantOf(item))
            }
            if (page.cursor === undefined) return grants
            cursor = page.cursor
          }
        }
      }),
    choices: async () =>
      choicesOf(await typed(async () => choicesFor(await connected()))),
    share: async (sessionId, request: ShareRequest, requestId) => {
      const answer = await typed(async () =>
        (await connected()).conversation.share(
          sessionId,
          request.credentialId,
          capabilitiesOf(request.role),
          request.policy ? wirePolicy(request.policy) : undefined,
          { requestId, expectedPolicyRevision: request.expectedPolicyRevision },
        ),
      )
      return answer.policyRevision
    },
    unshare: (sessionId, credentialId, expectedPolicyRevision, requestId) =>
      typed(async () => {
        await (
          await connected()
        ).conversation.unshare(sessionId, credentialId, {
          requestId,
          expectedPolicyRevision,
        })
      }),
  }
}
