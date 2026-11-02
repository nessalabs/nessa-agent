/**
 * The owner's remembered permission rules, for Settings › Access: the window's
 * client lists them and revokes one. A rule is one tool in one workspace.
 * Who may list or revoke is the gateway's to say, by its answers.
 */
import type { AppsApi, ConversationApi, PermissionRule } from "@nessa/client"

/** What Settings asks of the window's gateway client. */
export interface PermissionRulesClient {
  readonly conversation: Pick<ConversationApi, "permissionRules" | "revokePermissionRule">
  /** Absent on a client that cannot list apps; they then read as unread. */
  readonly apps?: Pick<AppsApi, "trusted" | "untrust">
}

/** The apps the person always allows, or the word that they could not be read. */
export type TrustedAppsRead =
  { readonly ok: true; readonly servers: readonly string[] } | { readonly ok: false }

/** A read, or the word that it could not be done. */
export type PermissionRulesRead =
  | { readonly ok: true; readonly rules: readonly PermissionRule[] }
  | { readonly ok: false }

/** A revoke, or the word that it could not be done. */
export type PermissionRulesRevoke =
  { readonly ok: true; readonly applied: boolean } | { readonly ok: false }

/** The remembered rules, as Settings › Access lists and revokes them. */
export interface PermissionRulesGateway {
  /** The stored rules. A refusal leaves the list unread. */
  read(): Promise<PermissionRulesRead>
  /**
   * Delete `ruleId`. `requestId` names this attempt. `applied` is false when
   * no rule had that id.
   */
  revoke(ruleId: string, requestId: string): Promise<PermissionRulesRevoke>
  /**
   * The apps answered "Always allow this app", by server. Absent where the
   * window cannot list them.
   */
  readonly trustedApps?: {
    read(): Promise<TrustedAppsRead>
    /** Stop always allowing `server`'s app; `applied` is false when it was not. */
    untrust(server: string, requestId: string): Promise<PermissionRulesRevoke>
  }
}

export function permissionRulesGateway(options: {
  connected: () => Promise<PermissionRulesClient>
}): PermissionRulesGateway {
  const conversation = () => options.connected().then((client) => client.conversation)
  return {
    read: async () => {
      try {
        const listed = await conversation().then((api) => api.permissionRules())
        return { ok: true, rules: listed.rules }
      } catch {
        return { ok: false }
      }
    },
    revoke: async (ruleId, requestId) => {
      try {
        const result = await conversation().then((api) =>
          api.revokePermissionRule(ruleId, { requestId }),
        )
        return { ok: true, applied: result.applied }
      } catch {
        return { ok: false }
      }
    },
    trustedApps: {
      read: async () => {
        try {
          const { apps } = await options.connected()
          if (!apps) return { ok: false }
          return { ok: true, servers: await apps.trusted() }
        } catch {
          return { ok: false }
        }
      },
      untrust: async (server, requestId) => {
        try {
          const { apps } = await options.connected()
          if (!apps) return { ok: false }
          return { ok: true, applied: await apps.untrust(server, requestId) }
        } catch {
          return { ok: false }
        }
      },
    },
  }
}
