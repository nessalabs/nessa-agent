/**
 * A session's composer shield, wired to the session's access
 * (`model/access.ts`): it shows what the store says — the person's choice
 * while it is the newer word, else the source's committed mode — and a pick
 * dispatches `chooseAccess`, so the shield never shows a mode nobody asked
 * the source for.
 */
import { useCallback, useMemo } from "react"
import type { ComposerAccessModeValue } from "@nessa-ui/react/composer-access-mode"
import type { AccessMode } from "../../model/access"
import { chooseAccess } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { useRememberedValue } from "../../adapters/store/remembered-session"
import { selectAccess, selectAccessNotice } from "../../adapters/store/selectors"
import type { WorkspaceFailureReason } from "../../model/failure"
import type { ComposerAccess } from "../../../ui/composer"
import { failureCopy } from "../failure-copy"

const shieldValue: Record<AccessMode, ComposerAccessModeValue> = {
  ask: "ask-approval",
  auto: "auto-approval",
  full: "full-access",
}

const accessMode: Record<ComposerAccessModeValue, AccessMode> = {
  "ask-approval": "ask",
  "auto-approval": "auto",
  "full-access": "full",
}

/** The shield's value for an access mode, and back: one table each way. */
export function shieldFor(mode: AccessMode): ComposerAccessModeValue {
  return shieldValue[mode]
}
export function accessFor(value: ComposerAccessModeValue): AccessMode {
  return accessMode[value]
}

/**
 * What the composer says when the source refused the shield's last choice:
 * a running turn in its own words, since the gateway changes access only
 * between turns, and any other refusal in the window's words for it.
 */
export function accessRefusalCopy(refusal: {
  readonly reason: WorkspaceFailureReason
  readonly code?: string
}): string {
  if (refusal.code === "turn_running")
    return "Access can’t change while the agent is working. Try again when it stops."
  if (refusal.code === "approval_mode_unavailable")
    return "This model doesn’t offer that access."
  return `Access didn’t change. ${failureCopy(refusal.reason)}`
}

/** What the composer says of a choice the agent's turn holds back. */
export const pendingAccessCopy = "Access changes when the agent stops."

/** The shield's value, its change handler, and what to say of its last choice, for a session's composer. */
export function useComposerAccess(sessionId: string): ComposerAccess {
  const dispatch = useWorkspaceDispatch()
  const modeLive = useWorkspaceSelector((state) => selectAccess(state, sessionId))
  const saidLive = useWorkspaceSelector((state) => selectAccessNotice(state, sessionId))
  const mode = useRememberedValue(sessionId, modeLive)
  const said = useRememberedValue(sessionId, saidLive)
  const onChange = useCallback(
    (value: ComposerAccessModeValue) =>
      void dispatch(
        chooseAccess({ sessionId, mode: accessFor(value), initiator: "person" }),
      ),
    [dispatch, sessionId],
  )
  return useMemo(() => {
    const notice: ComposerAccess["notice"] =
      said?.state === "pending"
        ? { tone: "neutral", text: pendingAccessCopy }
        : said?.state === "refused"
          ? { tone: "warning", text: accessRefusalCopy(said) }
          : undefined
    return { value: shieldFor(mode), onChange, ...(notice ? { notice } : {}) }
  }, [mode, onChange, said])
}
