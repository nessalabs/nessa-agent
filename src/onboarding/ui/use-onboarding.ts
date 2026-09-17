import * as React from "react"
import type { ShortcutsDocument } from "@nessa/client"
import defaults from "../../../protocol/defaults/shortcuts.v1.json"
import { host } from "../../host"
import { loadShortcuts } from "../../host/window"
import { formatAccelerator, summonAccelerator } from "../model/shortcut-display"
import {
  beginOnboarding,
  chooseAgent,
  completeOnboarding,
  confirmAgent,
  dismissOnboarding,
  isOnboarding,
  startAgentChoice,
  type AgentId,
  type OnboardingState,
} from "../model/onboarding"

const bundledDefaults = defaults as ShortcutsDocument

/** What the panel needs to paint first-run setup and move through it. */
export interface Onboarding {
  state: OnboardingState
  /** True while setup should be shown instead of the conversation. */
  active: boolean
  /**
   * The summon shortcut written for this platform, or undefined when the
   * configuration registers none. Setup says so rather than naming a shortcut
   * that would not work.
   */
  summon?: string
  begin: () => void
  choose: (id: AgentId) => void
  confirm: () => void
  finish: () => void
  /** Leave setup without finishing it. */
  dismiss: () => void
}

/**
 * Coordinates first-run setup for the panel.
 *
 * The chosen agent lives in memory for now: nothing is persisted and no
 * provider is configured, so a relaunch starts setup again. Persisting the
 * choice and connecting it to a gateway are separate steps; keeping them out
 * means this hook cannot imply an agent is ready to run.
 *
 * The summon shortcut is read from the host's own cache, falling back to the
 * bundled defaults the way the rest of the panel does, so setup teaches the
 * binding that is actually registered rather than a hardcoded one.
 */
export function useOnboarding(initial?: OnboardingState): Onboarding {
  const [state, setState] = React.useState<OnboardingState>(
    () => initial ?? beginOnboarding(),
  )
  const [shortcuts, setShortcuts] = React.useState<ShortcutsDocument>(bundledDefaults)

  React.useEffect(() => {
    let cancelled = false
    void loadShortcuts().then((loaded) => {
      if (!cancelled && loaded) setShortcuts(loaded)
    })
    return () => {
      cancelled = true
    }
  }, [])

  const keys = summonAccelerator(shortcuts)
  return {
    state,
    active: isOnboarding(state),
    summon: keys ? formatAccelerator(keys, host.kind) : undefined,
    begin: React.useCallback(() => setState(startAgentChoice), []),
    choose: React.useCallback(
      (id: AgentId) => setState((current) => chooseAgent(current, id)),
      [],
    ),
    confirm: React.useCallback(() => setState(confirmAgent), []),
    finish: React.useCallback(() => setState(completeOnboarding), []),
    dismiss: React.useCallback(() => setState(dismissOnboarding), []),
  }
}
