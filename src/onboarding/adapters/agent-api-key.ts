import { saveAgentApiKey } from "../../host"
import type { AgentApiKeySink } from "../application/ports"

/** Native secure-store adapter selected by desktop setup composition. */
export const nativeAgentApiKeys: AgentApiKeySink = { save: saveAgentApiKey }
