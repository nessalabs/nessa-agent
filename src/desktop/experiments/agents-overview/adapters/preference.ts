import { storedPreference } from "../../../adapters/stored-preference"
import { parseExperimentFlag } from "../model/experiment-flag"

/**
 * Settings › General › Experimental › Agents overview: whether the sidebar
 * offers the overview and ⌘0 opens it. Off unless turned on
 * (`parseExperimentFlag`); remembered like the window's other preferences,
 * and followed at once by every reader in the window.
 */
export const useAgentsOverviewPreference = storedPreference({
  key: "nessa.desktop.experiments.agents-overview",
  event: "nessa:desktop-experiments-agents-overview",
  parse: parseExperimentFlag,
}).usePreference
