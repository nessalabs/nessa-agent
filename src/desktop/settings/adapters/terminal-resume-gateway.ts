/**
 * Which terminal programs come back after Nessa restarts, for Settings ›
 * Terminal: the window's client (`client.terminals`), which checks each
 * answer against the protocol, read as it is.
 */
import type {
  TerminalApi,
  TerminalResumeSettings,
  TerminalResumeState,
} from "@nessa/client"

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
  /** Keep `settings`; answers with what the gateway kept, and the kept programs. */
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
    set: async (settings) => (await connected()).terminals.setResumeSettings(settings),
    forget: async (terminalId) => (await connected()).terminals.forgetSaved(terminalId),
  }
}
