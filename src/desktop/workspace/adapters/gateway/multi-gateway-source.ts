/**
 * Composite `WorkspaceSource` across 1..N gateways (ADR 1417, #1419).
 *
 * Qualifies every remote gateway's section, channel, session, and coordinator
 * IDs with `(gatewayId, rawId)` at this adapter boundary (G10), isolates
 * failures so an offline gateway never disturbs live gateways (G1), and routes
 * every mutation strictly to its owning gateway without cross-gateway fallback
 * (G2, G9).
 */
import {
  LOCAL_GATEWAY_ID,
  parseQualifiedId,
  qualifyEntityId,
  type GatewayConnectionState,
} from "../../model/gateway-id"
import type { AccessMode } from "../../model/access"
import type { ModelReceipt } from "../../model/model-choice"
import type { QuestionChoice, Transcript } from "../../model/transcript"
import type {
  Channel,
  ModelRef,
  Section,
  SessionSummary,
  WorkspaceIndex,
} from "../../model/workspace-index"
import {
  WorkspaceSourceError,
  type ApprovalScope,
  type Initiator,
  type OutgoingMessage,
  type WorkspaceSource,
  type WorkspaceUpdate,
} from "../../application/ports"

export interface ManagedWorkspaceGateway {
  readonly gatewayId: string
  readonly label: string
  readonly status: GatewayConnectionState
  readonly workspace: WorkspaceSource
}

export interface MultiGatewayWorkspaceBacking {
  /** Current gateways in display order (local first, then remotes). */
  entries(): readonly ManagedWorkspaceGateway[]
  /** Lookup one gateway by `gatewayId`. */
  entry(gatewayId: string): ManagedWorkspaceGateway | undefined
  /** Notified whenever the set of gateways, labels, or connection states changes. */
  onTopologyChange(listener: () => void): () => void
}

function qualifySection(
  gateway: ManagedWorkspaceGateway,
  section: Section,
  multi: boolean,
): Section {
  if (gateway.gatewayId === LOCAL_GATEWAY_ID) {
    return {
      ...section,
      name: multi ? gateway.label : section.name,
      gatewayId: LOCAL_GATEWAY_ID,
    }
  }
  return {
    id: qualifyEntityId(gateway.gatewayId, section.id),
    name: gateway.label,
    gatewayId: gateway.gatewayId,
  }
}

function qualifyChannel(gatewayId: string, channel: Channel): Channel {
  if (gatewayId === LOCAL_GATEWAY_ID) {
    return { ...channel, gatewayId: LOCAL_GATEWAY_ID }
  }
  return {
    ...channel,
    id: qualifyEntityId(gatewayId, channel.id),
    sectionId: qualifyEntityId(gatewayId, channel.sectionId),
    gatewayId,
    coordinatorConversationId:
      channel.coordinatorConversationId !== undefined
        ? qualifyEntityId(gatewayId, channel.coordinatorConversationId)
        : undefined,
  }
}

function qualifySession(gatewayId: string, session: SessionSummary): SessionSummary {
  if (gatewayId === LOCAL_GATEWAY_ID) {
    return { ...session, gatewayId: LOCAL_GATEWAY_ID }
  }
  return {
    ...session,
    id: qualifyEntityId(gatewayId, session.id),
    channelId: qualifyEntityId(gatewayId, session.channelId),
    gatewayId,
  }
}

function qualifyTranscript(gatewayId: string, transcript: Transcript): Transcript {
  if (gatewayId === LOCAL_GATEWAY_ID) return transcript
  return {
    ...transcript,
    sessionId: qualifyEntityId(gatewayId, transcript.sessionId),
  }
}

function qualifyUpdate(gatewayId: string, update: WorkspaceUpdate): WorkspaceUpdate {
  switch (update.kind) {
    case "session":
      return { kind: "session", session: qualifySession(gatewayId, update.session) }
    case "session-removed":
      return {
        kind: "session-removed",
        sessionId: qualifyEntityId(gatewayId, update.sessionId),
        revision: update.revision,
      }
    case "transcript":
      return {
        kind: "transcript",
        transcript: qualifyTranscript(gatewayId, update.transcript),
      }
    case "resync":
      return update
  }
}

function offlineError(status: GatewayConnectionState): WorkspaceSourceError {
  if (status === "auth-refused") {
    return new WorkspaceSourceError("signed-out")
  }
  return new WorkspaceSourceError("unavailable")
}

/**
 * Create a composite `WorkspaceSource` backed by all gateways in `backing`.
 */
export function multiGatewaySource(
  backing: MultiGatewayWorkspaceBacking,
): WorkspaceSource {
  function resolveOwner(qualifiedId: string): {
    gateway: ManagedWorkspaceGateway
    rawId: string
  } {
    const { gatewayId, rawId } = parseQualifiedId(qualifiedId)
    const gateway = backing.entry(gatewayId)
    if (!gateway) {
      throw new WorkspaceSourceError("unavailable")
    }
    if (
      gateway.gatewayId !== LOCAL_GATEWAY_ID &&
      (gateway.status === "auth-refused" ||
        gateway.status === "disconnected" ||
        gateway.status === "refused")
    ) {
      throw offlineError(gateway.status)
    }
    return { gateway, rawId }
  }

  return {
    async index(): Promise<WorkspaceIndex> {
      const entries = backing.entries()
      if (entries.length === 0) {
        return { sections: [], channels: [], sessions: [] }
      }
      const multi = entries.length > 1
      const settled = await Promise.allSettled(
        entries.map(async (gateway) => {
          // M4 / G1: never block local startup waiting on a remote gateway that
          // is not yet connected; once it connects, topology change emits resync.
          if (gateway.gatewayId !== LOCAL_GATEWAY_ID && gateway.status !== "connected") {
            throw offlineError(gateway.status)
          }
          const idx = await gateway.workspace.index()
          return { gateway, idx }
        }),
      )

      const sections: Section[] = []
      const channels: Channel[] = []
      const sessions: SessionSummary[] = []
      let succeeded = 0
      let firstError: unknown = null

      for (let at = 0; at < settled.length; at++) {
        const result = settled[at]
        const gateway = entries[at]
        if (result.status === "fulfilled") {
          succeeded += 1
          const { idx } = result.value
          for (const section of idx.sections) {
            sections.push(qualifySection(gateway, section, multi))
          }
          for (const channel of idx.channels) {
            channels.push(qualifyChannel(gateway.gatewayId, channel))
          }
          for (const session of idx.sessions) {
            sessions.push(qualifySession(gateway.gatewayId, session))
          }
        } else {
          if (firstError === null) firstError = result.reason
          // G1: keep an offline/connecting gateway visible as a section in the
          // sidebar so its status badge remains visible.
          sections.push({
            id: qualifyEntityId(gateway.gatewayId, "gateway"),
            name: gateway.label,
            gatewayId: gateway.gatewayId,
          })
        }
      }

      if (succeeded === 0 && firstError !== null) {
        throw firstError
      }
      return { sections, channels, sessions }
    },

    async transcript(sessionId: string): Promise<Transcript> {
      const { gateway, rawId } = resolveOwner(sessionId)
      const transcript = await gateway.workspace.transcript(rawId)
      return qualifyTranscript(gateway.gatewayId, transcript)
    },

    subscribe(listener: (update: WorkspaceUpdate) => void): () => void {
      const childUnsubs = new Map<
        string,
        { readonly source: WorkspaceSource; readonly unsub: () => void }
      >()
      let disposed = false

      const syncSubscriptions = () => {
        if (disposed) return
        const current = backing.entries()
        const activeIds = new Set(current.map((entry) => entry.gatewayId))

        for (const [gatewayId, held] of childUnsubs) {
          if (!activeIds.has(gatewayId)) {
            held.unsub()
            childUnsubs.delete(gatewayId)
          }
        }
        for (const entry of current) {
          const existing = childUnsubs.get(entry.gatewayId)
          // B2: re-pairing replaces the underlying WorkspaceSource instance under
          // the same gatewayId — unsubscribe the old one and bind the new one.
          if (existing && existing.source !== entry.workspace) {
            existing.unsub()
            childUnsubs.delete(entry.gatewayId)
          }
          if (!childUnsubs.has(entry.gatewayId)) {
            const unsub = entry.workspace.subscribe((update) => {
              if (!disposed) {
                listener(qualifyUpdate(entry.gatewayId, update))
              }
            })
            childUnsubs.set(entry.gatewayId, { source: entry.workspace, unsub })
          }
        }
      }

      syncSubscriptions()
      const unsubTopology = backing.onTopologyChange(() => {
        if (disposed) return
        syncSubscriptions()
        listener({ kind: "resync" })
      })

      return () => {
        disposed = true
        unsubTopology()
        for (const held of childUnsubs.values()) {
          held.unsub()
        }
        childUnsubs.clear()
      }
    },

    async send(message: OutgoingMessage): Promise<void> {
      // A brand-new draft uses a local UUID for `sessionId`, so route by
      // `start.channelId` when starting a session in a remote channel.
      const routingId = message.start ? message.start.channelId : message.sessionId
      const { gateway } = resolveOwner(routingId)
      const parsedSession = parseQualifiedId(message.sessionId)
      const rawSessionId =
        parsedSession.gatewayId === gateway.gatewayId
          ? parsedSession.rawId
          : message.sessionId
      const rawMessage: OutgoingMessage = {
        ...message,
        sessionId: rawSessionId,
        start: message.start
          ? {
              ...message.start,
              channelId: parseQualifiedId(message.start.channelId).rawId,
            }
          : undefined,
      }
      await gateway.workspace.send(rawMessage)
    },

    async approve(
      sessionId: string,
      approvalId: string,
      scope: ApprovalScope,
      initiator: Initiator,
      optionId: string,
    ): Promise<void> {
      const { gateway, rawId } = resolveOwner(sessionId)
      await gateway.workspace.approve(rawId, approvalId, scope, initiator, optionId)
    },

    async deny(
      sessionId: string,
      approvalId: string,
      initiator: Initiator,
      optionId: string,
    ): Promise<void> {
      const { gateway, rawId } = resolveOwner(sessionId)
      await gateway.workspace.deny(rawId, approvalId, initiator, optionId)
    },

    retryReview(sessionId: string, approvalId: string): void {
      const { gatewayId, rawId } = parseQualifiedId(sessionId)
      backing.entry(gatewayId)?.workspace.retryReview?.(rawId, approvalId)
    },

    async loadEarlier(sessionId: string): Promise<boolean> {
      const { gateway, rawId } = resolveOwner(sessionId)
      return gateway.workspace.loadEarlier ? gateway.workspace.loadEarlier(rawId) : false
    },

    async answerQuestion(
      sessionId: string,
      executionId: string,
      questionId: string,
      choices: readonly QuestionChoice[] | null,
      initiator: Initiator,
      key?: string,
    ): Promise<void> {
      const { gateway, rawId } = resolveOwner(sessionId)
      await gateway.workspace.answerQuestion(
        rawId,
        executionId,
        questionId,
        choices,
        initiator,
        key,
      )
    },

    async stop(sessionId: string, initiator: Initiator): Promise<void> {
      const { gateway, rawId } = resolveOwner(sessionId)
      await gateway.workspace.stop(rawId, initiator)
    },

    async setModel(
      sessionId: string,
      model: ModelRef,
      requestId: string,
      initiator: Initiator,
    ): Promise<ModelReceipt> {
      const { gateway, rawId } = resolveOwner(sessionId)
      return gateway.workspace.setModel(rawId, model, requestId, initiator)
    },

    async setAccess(
      sessionId: string,
      mode: AccessMode,
      initiator: Initiator,
    ): Promise<void> {
      const { gateway, rawId } = resolveOwner(sessionId)
      await gateway.workspace.setAccess(rawId, mode, initiator)
    },

    async setUsageLimitAutoContinue(
      enabled: boolean,
      initiator: Initiator,
    ): Promise<void> {
      const local = backing.entry(LOCAL_GATEWAY_ID)
      if (!local) throw new WorkspaceSourceError("unavailable")
      await local.workspace.setUsageLimitAutoContinue(enabled, initiator)
    },

    async setPinned(
      sessionId: string,
      pinned: boolean,
      initiator: Initiator,
    ): Promise<void> {
      const { gateway, rawId } = resolveOwner(sessionId)
      await gateway.workspace.setPinned(rawId, pinned, initiator)
    },

    async archive(sessionId: string, initiator: Initiator): Promise<void> {
      const { gateway, rawId } = resolveOwner(sessionId)
      await gateway.workspace.archive(rawId, initiator)
    },

    async rename(sessionId: string, title: string, initiator: Initiator): Promise<void> {
      const { gateway, rawId } = resolveOwner(sessionId)
      await gateway.workspace.rename(rawId, title, initiator)
    },

    async fork(sessionId: string, initiator: Initiator): Promise<string> {
      const { gateway, rawId } = resolveOwner(sessionId)
      const forkedRawId = await gateway.workspace.fork(rawId, initiator)
      return qualifyEntityId(gateway.gatewayId, forkedRawId)
    },

    async delete(sessionId: string, initiator: Initiator): Promise<void> {
      const { gateway, rawId } = resolveOwner(sessionId)
      await gateway.workspace.delete(rawId, initiator)
    },

    async markRead(sessionId: string): Promise<void> {
      const { gateway, rawId } = resolveOwner(sessionId)
      await gateway.workspace.markRead(rawId)
    },

    keepFollowing(sessionId: string | null): void {
      if (sessionId === null) {
        for (const entry of backing.entries()) {
          entry.workspace.keepFollowing(null)
        }
        return
      }
      const { gatewayId, rawId } = parseQualifiedId(sessionId)
      for (const entry of backing.entries()) {
        entry.workspace.keepFollowing(entry.gatewayId === gatewayId ? rawId : null)
      }
    },
  }
}
