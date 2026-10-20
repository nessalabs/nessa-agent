/**
 * The usage-limit card's checkbox: this gateway's preference for scheduling
 * one continuation when a limit resets. The conversation that says so reaches
 * subscribers as an update. The gateway records the change.
 */
import type { Initiator } from "../../application/ports"
import type { WorkspaceCommand } from "./commands"

export function setUsageLimitAutoContinue({
  enabled,
  initiator,
}: {
  enabled: boolean
  initiator: Initiator
}): WorkspaceCommand<Promise<void>> {
  return async (_dispatch, _getState, { workspace }) => {
    await workspace.setUsageLimitAutoContinue(enabled, initiator)
  }
}
