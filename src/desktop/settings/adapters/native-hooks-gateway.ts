/**
 * Claude and Codex native-hook choices, for Settings › Access: the window's
 * client (`client.agents`) read into two switches. OpenCode is not a choice.
 * Who may change the choice is the gateway's to say, by its answers.
 */
import type { AgentsApi, NativeHooksAgent } from "@nessa/client"

/** What Settings asks of the window's gateway client. */
export interface NativeHooksClient {
  readonly agents: Pick<AgentsApi, "list" | "setNativeHooks">
}

/** The choice for one harness the caller can change. */
export type NativeHooksAgentName = NativeHooksAgent

/** What the next new conversation of Claude and Codex will do. True allows hooks. */
export interface NativeHooksChoice {
  readonly claude: boolean
  readonly codex: boolean
  /** False when that agent is not in the catalog. Absent means it is. */
  readonly claudeConfigured?: boolean
  readonly codexConfigured?: boolean
}

/** A read, or the word that it could not be done. */
export type NativeHooksRead =
  { readonly ok: true; readonly choice: NativeHooksChoice } | { readonly ok: false }

/** A change, or the word that it could not be done. */
export type NativeHooksWrite =
  | { readonly ok: true; readonly agent: NativeHooksAgentName; readonly enabled: boolean }
  | { readonly ok: false }

/** The two switches, as Settings › Access manages them. */
export interface NativeHooksGateway {
  /** The stored choice. A missing catalog row does not change it. */
  read(): Promise<NativeHooksRead>
  /**
   * Change one harness. `requestId` names this attempt. A refusal leaves the
   * previous choice in effect.
   */
  set(
    agent: NativeHooksAgentName,
    enabled: boolean,
    requestId: string,
  ): Promise<NativeHooksWrite>
}

/** The stored choice, and whether each harness is in the catalog. */
function selection(
  listed: Awaited<ReturnType<AgentsApi["list"]>>,
): NativeHooksChoice | undefined {
  const hooks = listed.nativeHooks
  if (
    !hooks ||
    !Object.hasOwn(hooks, "claude") ||
    !Object.hasOwn(hooks, "codex") ||
    (hooks.claude !== "off" && hooks.claude !== "on") ||
    (hooks.codex !== "off" && hooks.codex !== "on")
  )
    return undefined
  const names = new Set(listed.agents.map((agent) => agent.agent))
  return {
    claude: hooks.claude === "on",
    codex: hooks.codex === "on",
    claudeConfigured: names.has("claude"),
    codexConfigured: names.has("codex"),
  }
}

export function nativeHooksGateway(options: {
  connected: () => Promise<NativeHooksClient>
}): NativeHooksGateway {
  const agents = () => options.connected().then((client) => client.agents)
  return {
    read: async () => {
      try {
        const listed = await agents().then((api) => api.list())
        const choice = selection(listed)
        if (!choice) return { ok: false }
        return { ok: true, choice }
      } catch {
        return { ok: false }
      }
    },
    set: async (agent, enabled, requestId) => {
      try {
        const result = await agents().then((api) =>
          api.setNativeHooks(agent, enabled, requestId),
        )
        return { ok: true, agent: result.agent, enabled: result.enabled }
      } catch {
        return { ok: false }
      }
    },
  }
}
