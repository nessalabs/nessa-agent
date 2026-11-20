/**
 * How the person's decisions behave, for Settings › Chat settings ›
 * Decisions: the window's client (`client.conversation`), which checks each
 * answer against the protocol, read as it is.
 */
import type { ConversationApi, DecisionSettings } from "@nessa/client"

/** What Settings asks of the window's gateway client. */
export interface DecisionSettingsClient {
  readonly conversation: Pick<ConversationApi, "decisionSettings" | "setDecisionSettings">
}

/** Auto-pick, as Settings reads and saves it. */
export interface DecisionSettingsGateway {
  read(): Promise<DecisionSettings>
  /** Keep `settings`; answers with what the gateway kept. */
  set(settings: DecisionSettings): Promise<DecisionSettings>
}

export function decisionSettingsGateway({
  connected,
}: {
  connected: () => Promise<DecisionSettingsClient>
}): DecisionSettingsGateway {
  return {
    read: async () => (await connected()).conversation.decisionSettings(),
    set: async (settings) =>
      (await connected()).conversation.setDecisionSettings(settings),
  }
}
