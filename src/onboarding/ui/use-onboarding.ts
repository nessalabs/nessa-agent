import * as React from "react"
import type { ShortcutsDocument } from "@nessa/client"
import defaults from "../../../protocol/defaults/shortcuts.v1.json"
import { host } from "../../host"
import { matchesAccelerator } from "../../host/accelerator"
import type { ShortcutPlatform } from "../model/shortcut-display"
import { loadShortcuts } from "../../host/window"
import { summonAccelerator } from "../model/shortcut-display"
import {
  beginOnboarding,
  chooseAgent,
  completeOnboarding,
  confirmAgent,
  dismissOnboarding,
  confirmSummon,
  isOnboarding,
  startAgentChoice,
  type AgentId,
  type OnboardingState,
} from "../model/onboarding"

const bundledDefaults = defaults as ShortcutsDocument

/**
 * Which keyboard conventions to write a shortcut in.
 *
 * A native host already knows what it is. A browser does not report the host,
 * so it is asked about the machine it runs on, which is what decides whether
 * `CmdOrCtrl` is read as Command or Control.
 */
function shortcutPlatform(): ShortcutPlatform {
  if (host.kind === "macos") return "apple"
  if (host.kind === "linux") return "linux"
  const agent =
    typeof navigator === "undefined"
      ? ""
      : `${navigator.userAgent} ${navigator.platform ?? ""}`
  if (/mac|iphone|ipad|ipod/i.test(agent)) return "apple"
  if (/linux|x11|cros/i.test(agent)) return "linux"
  return "windows"
}

/** What the panel needs to paint first-run setup and move through it. */
export interface Onboarding {
  state: OnboardingState
  /** True while setup should be shown instead of the conversation. */
  active: boolean
  /**
   * The summon accelerator as the host registers it, or undefined when the
   * configuration registers none. Setup says so rather than naming a shortcut
   * that would not work.
   */
  summon?: string
  /** The keyboard conventions this device writes shortcuts in. */
  platform: ShortcutPlatform
  begin: () => void
  choose: (id: AgentId) => void
  confirm: () => void
  finish: () => void
  /** Leave setup without finishing it. */
  dismiss: () => void
  /** Move on from the summon step, for hosts where the shortcut never reaches
   * this window. */
  confirmSummon: () => void
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
  const platform = shortcutPlatform()

  // While setup is teaching the summon shortcut, pressing it moves on. The
  // desktop host also holds this accelerator globally, so on that host the press
  // may be taken by the global binding before the window sees it; the step still
  // moves on from its button.
  const practising = state.step === "summon"
  React.useEffect(() => {
    if (!practising || !keys) return
    function onKeyDown(event: KeyboardEvent) {
      if (event.repeat || !matchesAccelerator(event, keys)) return
      event.preventDefault()
      setState(confirmSummon)
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [practising, keys])

  return {
    state,
    active: isOnboarding(state),
    summon: keys,
    platform,
    begin: React.useCallback(() => setState(startAgentChoice), []),
    choose: React.useCallback(
      (id: AgentId) => setState((current) => chooseAgent(current, id)),
      [],
    ),
    confirm: React.useCallback(() => setState(confirmAgent), []),
    finish: React.useCallback(() => setState(completeOnboarding), []),
    dismiss: React.useCallback(() => setState(dismissOnboarding), []),
    confirmSummon: React.useCallback(() => setState(confirmSummon), []),
  }
}
