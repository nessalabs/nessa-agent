import { useState } from "react"
import { useClipboardWrite } from "../../../../conversation"
import { DesktopIcon } from "../../../ui/icons"
import { discardUnsent, resendMessage } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { entry } from "../../application/workspace-state"
import { setupFailedFor } from "../../model/machine-setup"
import type { TurnMessage } from "../../model/transcript"
import { failureCopy, unsentLabel } from "../failure-copy"

/**
 * Why the person's message did not go, right under it, in two or three
 * words, then Send again and Dismiss, and a copy button that holds the whole
 * reason for a report. The composer below says nothing of it.
 */
export function UnsentRow({
  sessionId,
  message,
}: {
  sessionId: string
  message: TurnMessage
}) {
  const dispatch = useWorkspaceDispatch()
  const write = useClipboardWrite()
  const setups = useWorkspaceSelector(
    (state) => entry(state.workspace.transcripts, sessionId)?.setups,
  )
  // Whether the gateway took any message of this chat's: until it has, the
  // chat's own creation is what its first message waited on.
  const answered = useWorkspaceSelector(
    (state) =>
      entry(state.workspace.transcripts, sessionId)?.messages.some(
        (each) => each.role === "user" && each.delivery === undefined,
      ) ?? false,
  )
  const [copied, setCopied] = useState<"done" | "failed" | null>(null)
  if (message.delivery?.state !== "failed") return null
  const { reason, diagnostic } = message.delivery
  const label = unsentLabel(
    reason,
    diagnostic?.code,
    setupFailedFor(setups, message.id, answered),
  )
  const details = diagnostic?.message ?? failureCopy(reason)
  const report = diagnostic?.code ? `${diagnostic.code}\n${details}` : details
  const copyLabel = copied === "failed" ? "Copy failed; try again" : "Copy error details"
  return (
    <div className="workspace-unsent" role="status" data-unsent="">
      <span className="workspace-unsent-why">{label}</span>
      <button
        type="button"
        className="workspace-unsent-action"
        data-unsent-retry=""
        onClick={() =>
          void dispatch(
            resendMessage({ sessionId, messageId: message.id, initiator: "person" }),
          )
        }
      >
        Send again
      </button>
      <button
        type="button"
        className="workspace-unsent-action"
        data-unsent-dismiss=""
        onClick={() => dispatch(discardUnsent({ sessionId, messageId: message.id }))}
      >
        Dismiss
      </button>
      <span className="workspace-unsent-rule" aria-hidden="true" />
      <button
        type="button"
        className="workspace-unsent-action"
        data-unsent-copy=""
        data-copied={copied === "done" || undefined}
        aria-label={copyLabel}
        title={copyLabel}
        disabled={!write}
        onClick={async () => {
          if (!write) return
          try {
            await write(report)
            setCopied("done")
          } catch {
            setCopied("failed")
          }
        }}
      >
        <DesktopIcon name={copied === "done" ? "check" : "copy"} />
      </button>
    </div>
  )
}
