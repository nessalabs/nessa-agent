/**
 * What the subagents plugin answers for one conversation, in one order:
 * the preview off, then a conversation the workspace does not list once
 * its index is read, then the index or the source not read yet, then ready.
 * `widget-state.test.ts` holds that order.
 */
import type { Flag } from "../../model/window-preferences"
import type { WidgetState } from "../../widgets/model/widget-state"
import type { SessionListing } from "../../workspace/application/workspace-state"
import type { SubagentRead } from "./ports"

export function subagentsWidgetState(input: {
  readonly sessionId: string
  readonly preview: Flag
  readonly listing: SessionListing
  readonly read: SubagentRead
}): WidgetState {
  if (input.preview === "off") return { kind: "off" }
  if (input.listing === "absent") return { kind: "missing" }
  if (input.listing === "unread" || input.read.kind === "unread")
    return { kind: "unread" }
  return { kind: "ready", title: "Subagents", origin: input.sessionId }
}
