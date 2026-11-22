/**
 * A session's composer shield, wired to the session's access
 * (`model/access.ts`): it shows what the store says — the person's choice
 * while it is the newer word, else the source's committed mode — and a pick
 * dispatches `chooseAccess`, so the shield never shows a mode nobody asked
 * the source for.
 */
import { useCallback, useMemo } from "react"
import {
  composerAccessModeLabels,
  type ComposerAccessModeValue,
} from "@nessa-ui/react/composer-access-mode"
import type { AccessMode } from "../../model/access"
import { chooseAccess } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { useRememberedValue } from "../../adapters/store/remembered-session"
import {
  selectAccess,
  selectAccessNotice,
  selectSteppedAccess,
} from "../../adapters/store/selectors"
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
 * What the shield says when the source refused its last choice:
 * a running turn in its own words, and any other refusal in the window's
 * words for it.
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

/**
 * What the shield says when a model change moved the chat to Ask, because
 * the new model is not offered at its access here.
 */
export function steppedAccessCopy(from: AccessMode): string {
  return `Access is now ${composerAccessModeLabels[shieldFor("ask")]}. This model doesn’t offer ${composerAccessModeLabels[shieldFor(from)]} here.`
}

/** The shield's value, its change handler, and what it says of its last choice, for a session's composer. */
export function useComposerAccess(sessionId: string): ComposerAccess {
  const dispatch = useWorkspaceDispatch()
  const modeLive = useWorkspaceSelector((state) => selectAccess(state, sessionId))
  const saidLive = useWorkspaceSelector((state) => selectAccessNotice(state, sessionId))
  const mode = useRememberedValue(sessionId, modeLive)
  const said = useRememberedValue(sessionId, saidLive)
  const steppedLive = useWorkspaceSelector((state) =>
    selectSteppedAccess(state, sessionId),
  )
  const stepped = useRememberedValue(sessionId, steppedLive)
  const onChange = useCallback(
    (value: ComposerAccessModeValue) =>
      void dispatch(
        chooseAccess({ sessionId, mode: accessFor(value), initiator: "person" }),
      ),
    [dispatch, sessionId],
  )
  return useMemo(() => {
    // Only a refusal is said, by the shield, because then its choice did
    // not happen. A choice held back (one the running turn could not take
    // now, or one queued behind a change on its way) says nothing: the shield
    // already shows it. A model change that moved the chat to Ask says so,
    // since nobody picked Ask. Neither is a line in the composer.
    const note: ComposerAccess["note"] =
      said?.state === "refused"
        ? { tone: "warning", text: accessRefusalCopy(said) }
        : stepped
          ? { tone: "neutral", text: steppedAccessCopy(stepped) }
          : undefined
    return { value: shieldFor(mode), onChange, ...(note ? { note } : {}) }
  }, [mode, onChange, said, stepped])
}
