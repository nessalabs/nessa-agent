/**
 * Which terminal programs come back after Nessa restarts, for Settings ›
 * Terminal: the window's client (`client.terminals`), which checks each
 * answer against the protocol, read as it is.
 */
import {
  NessaRpcError,
  type TerminalApi,
  type TerminalResumeSettings,
  type TerminalResumeState,
} from "@nessa/client"
import { RuleRefused, type RefusedPart } from "../model/terminal-resume"

/** What Settings asks of the window's gateway client. */
export interface TerminalResumeClient {
  readonly terminals: Pick<
    TerminalApi,
    "resumeSettings" | "setResumeSettings" | "forgetSaved"
  >
}

/** Resume settings and kept programs, as Settings reads and changes them. */
export interface TerminalResumeGateway {
  read(): Promise<TerminalResumeState>
  /**
   * Keep `settings`; answers with what the gateway kept, and the kept
   * programs. Rejects with {@link RuleRefused} when the gateway refuses a rule.
   */
  set(settings: TerminalResumeSettings): Promise<TerminalResumeState>
  /** Forget one kept program; answers with what is kept now. */
  forget(terminalId: string): Promise<TerminalResumeState>
}

export function terminalResumeGateway({
  connected,
}: {
  connected: () => Promise<TerminalResumeClient>
}): TerminalResumeGateway {
  return {
    read: async () => (await connected()).terminals.resumeSettings(),
    set: async (settings) => {
      const client = await connected()
      try {
        return await client.terminals.setResumeSettings(settings)
      } catch (cause) {
        const part = refusedPart(cause)
        throw part ? new RuleRefused(part) : cause
      }
    },
    forget: async (terminalId) => (await connected()).terminals.forgetSaved(terminalId),
  }
}

/** The part a gateway refusal names, when it is a refused rule. */
function refusedPart(cause: unknown): RefusedPart | undefined {
  if (!(cause instanceof NessaRpcError)) return undefined
  if (cause.code === "terminal_resume_rules_limit") return "count"
  if (cause.code !== "terminal_resume_rule_invalid") return undefined
  const field =
    typeof cause.details === "object" && cause.details !== null
      ? (cause.details as { field?: unknown }).field
      : undefined
  return field === "program" || field === "folder" || field === "args" ? field : "program"
}
