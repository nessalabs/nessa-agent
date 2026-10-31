import { parseTranscriptView } from "../../model/transcript-view"
import { storedPreference } from "../../../adapters/stored-preference"

/**
 * How much of the agent's work conversations show (`transcript-view.ts`),
 * remembered like the window's other preferences and shared by every pane,
 * so choosing it from one pane's menu changes them all.
 */
const transcriptViewPreference = storedPreference({
  key: "nessa.desktop.transcript-view",
  event: "nessa:desktop-transcript-view",
  parse: parseTranscriptView,
})

export const useTranscriptViewPreference = transcriptViewPreference.usePreference
