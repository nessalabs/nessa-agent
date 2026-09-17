import * as React from "react"
import type { ShortcutsDocument } from "@nessa/client"
import defaults from "../../../protocol/defaults/shortcuts.v1.json"
import { host } from "../../host"
import { matchesAccelerator } from "../../host/accelerator"
import { playCue } from "./sound"
import type { ShortcutPlatform } from "../model/shortcut-display"
import { loadShortcuts, onSummoned } from "../../host/window"
import { summonAccelerator } from "../model/shortcut-display"
import {
  beginOnboarding,
  chooseAgent,
  completeOnboarding,
  confirmAgent,
  dismissOnboarding,
  confirmSummon,
  isOnboarding,
  pressSummon,
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
  accelerator?: string
  /** The keyboard conventions this device writes shortcuts in. */
  platform: ShortcutPlatform
  begin: () => void
  choose: (id: AgentId) => void
  confirm: () => void
  finish: () => void
  /** Leave setup without finishing it. */
  dismiss: () => void
  /** Move on from the summon step, once the shortcut has been pressed. */
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

  // Setup has no chrome and no visible way out — a close control on the wash
  // read as a blemish on it — so Escape is the way out, which is what Escape
  // means on a modal surface anyway. It is bound for the whole of setup rather
  // than one step: someone who wants to leave will not have read this far.
  React.useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape" || event.repeat) return
      event.preventDefault()
      setState(dismissOnboarding)
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [])

  // While setup is teaching the summon shortcut, pressing it satisfies the
  // step: the keys light up and the way on appears. It does not move on by
  // itself, so the press is something a person sees land rather than a screen
  // that vanishes under them.
  const practising = state.step === "summon"
  // Which half of the lesson a press would be, so the cue can be chosen before
  // the state changes rather than from inside an updater, which React is free
  // to run more than once.
  const lesson = state.step === "summon" ? state.summon : undefined
  React.useEffect(() => {
    // Only where nothing else reports the press. A native host holds this
    // accelerator with the system and tells us about it, and counting both a
    // key and a report would make one press look like two.
    if (!practising || !keys || host.kind !== "browser") return
    function onKeyDown(event: KeyboardEvent) {
      if (event.repeat || !matchesAccelerator(event, keys)) return
      event.preventDefault()
      if (lesson === "hidden") return
      playCue(lesson === undefined ? "summon" : "dismiss")
      setState(pressSummon)
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [practising, keys, lesson])

  // The desktop host registers this accelerator with the system, so the press
  // is taken before this window sees a key — which is why it also reports the
  // summon it handled. That report is what makes the lesson work on the one
  // platform that ships: the panel really does appear and disappear, and the
  // keys light because it did.
  React.useEffect(() => {
    if (!practising) return
    let stop: (() => void) | undefined
    let cancelled = false
    void onSummoned((showing) => {
      playCue(showing ? "summon" : "dismiss")
      setState(pressSummon)
    }).then((unlisten) => {
      if (cancelled) unlisten()
      else stop = unlisten
    })
    return () => {
      cancelled = true
      stop?.()
    }
  }, [practising])

  return {
    state,
    active: isOnboarding(state),
    accelerator: keys,
    platform,
    // Every button that moves setup forward sounds the same, because each is
    // the same act. Picking an agent is not one of them — it switches a thing
    // on — and finishing gets the only celebratory cue setup has.
    begin: React.useCallback(() => {
      playCue("advance")
      setState(startAgentChoice)
    }, []),
    choose: React.useCallback((id: AgentId) => {
      playCue("choose")
      setState((current) => chooseAgent(current, id))
    }, []),
    confirm: React.useCallback(() => {
      playCue("advance")
      setState(confirmAgent)
    }, []),
    finish: React.useCallback(() => {
      playCue("celebrate")
      setState(completeOnboarding)
    }, []),
    // Leaving is not an accomplishment and does not announce itself.
    dismiss: React.useCallback(() => setState(dismissOnboarding), []),
    confirmSummon: React.useCallback(() => {
      playCue("advance")
      setState(confirmSummon)
    }, []),
  }
}
