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

/** What the next launch of Claude and Codex will do. True omits suppression. */
export interface NativeHooksChoice {
  readonly claude: boolean
  readonly codex: boolean
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
  /** The choice in effect. A catalog row that omits the flag is off. */
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

/** The choice a catalog row publishes. Absent is off. */
function flag(
  agents: Awaited<ReturnType<AgentsApi["list"]>>,
  name: NativeHooksAgentName,
): boolean {
  const row = agents.agents.find((agent) => agent.agent === name)
  return row?.nativeHooks === true
}

export function nativeHooksGateway(options: {
  connected: () => Promise<NativeHooksClient>
}): NativeHooksGateway {
  const agents = () => options.connected().then((client) => client.agents)
  return {
    read: async () => {
      try {
        const listed = await agents().then((api) => api.list())
        return {
          ok: true,
          choice: { claude: flag(listed, "claude"), codex: flag(listed, "codex") },
        }
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
