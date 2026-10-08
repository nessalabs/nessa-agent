/**
 * One face in an `AvatarStack`. An open child that is working is busy, and
 * its name says so (`subagent-stack.test.tsx`): the stack's motion is not
 * the only way that state is read. The panel heading and the pane header
 * both call this.
 */
import type { AvatarStackItem } from "@nessa-ui/react/avatar-stack"
import type { Subagent } from "../model/subagent"

export function stackItem(subagent: Subagent): AvatarStackItem {
  const working = subagent.lifecycle === "open" && subagent.activity === "working"
  return {
    seed: subagent.seed,
    name: working ? `${subagent.name}, working` : subagent.name,
    busy: working,
  }
}
