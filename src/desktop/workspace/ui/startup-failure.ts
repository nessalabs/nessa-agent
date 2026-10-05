import { startupCode } from "../../../host/startup-refusals"
import type { WorkspaceFailureReason } from "../model/failure"

/**
 * The startup failures the window covers with the calm screen. Anything else
 * the workspace can fail as stays in the empty pane.
 */
export function startupFailureCode(reason: WorkspaceFailureReason | null): string | null {
  switch (reason) {
    case "not-started":
      return startupCode("not-provisioned")
    case "not-ready":
      return startupCode("not-ready")
    case "not-listening":
      return startupCode("not-listening")
    case "wrong-stage":
      return startupCode("wrong-stage")
    default:
      return null
  }
}
