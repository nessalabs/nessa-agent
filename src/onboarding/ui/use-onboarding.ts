import * as React from "react"
import type { ShortcutsDocument } from "@nessa/client"
import defaults from "../../../protocol/defaults/shortcuts.v1.json"
import { host, loadShortcuts, matchesAccelerator, onSummoned } from "../../host"
import { nativeGatewayStartup } from "../../startup/adapters/gateway-startup"
import {
  createGatewayStartupMonitor,
  type GatewayStartupStatus,
} from "../../startup/application/gateway-startup"
import { playCue } from "./sound"
import { listenForDismiss } from "./dismiss-shortcut"
import { createReadinessCheck } from "../application/readiness-check"
import type { ShortcutPlatform } from "../model/shortcut-display"
import type { AgentReadinessSource } from "../application/ports"
import { summonAccelerator } from "../model/shortcut-display"
import {
  beginOnboarding,
  clearReadiness,
  chooseAgent,
  completeOnboarding,
  confirmAgent,
  dismissOnboarding,
  isOnboarding,
  pressSummon,
  recordReadiness,
  recordReadinessFailure,
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
  /** What the native host reports about bringing up its managed gateway. */
  gatewayStartup: GatewayStartupStatus
  begin: () => void
  choose: (id: AgentId) => void
  confirm: () => void
  finish: () => void
  /** Leave setup without finishing it. */
  dismiss: () => void
  /** Ask the runtimes again after an agent was installed or signed in. Does
   * nothing while an ask is already in flight, or while the managed gateway is
   * not ready to answer. */
  recheck: () => void
  /** Ask the native host to retry a failed gateway startup. */
  retryGatewayStartup: () => void
  /** True while an ask is in flight, so the control that starts one can show
   * that it is working rather than looking like it did nothing. */
  checking: boolean
}

/**
 * Coordinates first-run setup for the panel.
 *
 * Finishing setup is recorded in the state and nowhere else. Persisting it with
 * the host — so a relaunch opens straight into the panel — belongs to whoever
 * hands over to the panel, because it must not outrun a handoff that fails.
 * The same is true of the chosen agent, which this hook also only holds: it
 * decides which agent every conversation runs on, and it is carried out of here
 * by the handover — to the host on the desktop, and to the panel's own
 * dependencies on a surface that has no host. Keeping the writing out means
 * this hook cannot imply a choice was kept when the handoff it travels with
 * failed.
 *
 * The summon shortcut is read from the host's own cache, falling back to the
 * bundled defaults the way the rest of the panel does, so setup teaches the
 * binding that is actually registered rather than a hardcoded one.
 */
export function useOnboarding(
  agents: AgentReadinessSource,
  initial?: OnboardingState,
): Onboarding {
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

  // What each agent's runtime reports. Nothing is offered until the answer
  // arrives: an agent is not choosable on the strength of not having been asked
  // about. Not getting an answer is recorded as such, so the picker can say the
  // gateway is unreachable rather than blaming the agent for it.
  //
  // The ask is repeatable, because every reason it can fail is a reason that
  // goes away: a gateway still reconciling when the window opened comes up, and
  // an agent that needs signing in gets signed in — in another window, while
  // this one is watching. Only the newest ask's answer is kept (see
  // `createReadinessCheck`), so a slow first answer cannot land on top of a
  // fresh one.
  //
  // One ask at a time, and that too is `createReadinessCheck`'s: every ask
  // reaches the readiness probe, which does real work on the machine — on macOS
  // a blocking subprocess per agent — so a person pressing "Check again" while
  // nothing visibly happens would otherwise stack up probes. Which ask is
  // current and whether one is outstanding live together there, so abandoning
  // an ask on the way out releases both; a busy flag owned out here could
  // refuse the next ask on behalf of one whose answer had already been dropped.
  // This hook only paints what that object reports.
  const [checking, setChecking] = React.useState(false)
  const readiness = React.useMemo(
    () =>
      createReadinessCheck(
        agents,
        (answer) =>
          setState((current) =>
            answer.ok
              ? recordReadiness(current, answer.agents)
              : recordReadinessFailure(current, answer.reason),
          ),
        setChecking,
      ),
    [agents],
  )
  const ask = React.useCallback(() => {
    void readiness.check()
  }, [readiness])
  const [gatewayStartupState, setGatewayStartupState] =
    React.useState<GatewayStartupStatus>({
      revision: -1,
      state: "starting",
      step: "preparing",
    })
  const startup = React.useMemo(
    () =>
      createGatewayStartupMonitor(nativeGatewayStartup, (next) => {
        setGatewayStartupState(next)
        if (next.state === "ready") {
          // A changed native identity may advance directly from Ready to a
          // newer Ready when its invalidation event was missed. Its readiness
          // answer still belongs to the old identity, so replace even an
          // outstanding ask before probing the newly confirmed gateway.
          readiness.abandon()
          setState(clearReadiness)
          void readiness.check()
          return
        }
        if (next.state === "unmanaged") {
          void readiness.check()
          return
        }
        readiness.abandon()
        setState(clearReadiness)
      }),
    [readiness],
  )

  React.useEffect(() => {
    startup.start()
    return () => {
      startup.stop()
      readiness.abandon()
    }
  }, [readiness, startup])

  const recheck = React.useCallback(() => {
    if (
      gatewayStartupState.state === "ready" ||
      gatewayStartupState.state === "unmanaged"
    ) {
      ask()
    }
  }, [ask, gatewayStartupState.state])
  const retryStartup = React.useCallback(() => {
    void startup.retry().catch(() => undefined)
  }, [startup])

  // The unmanaged browser path keeps its original second ask on reaching the
  // picker. It has no native lifecycle event to trigger a fresh answer, and the
  // list is about to be acted on. A managed host asks once on its confirmed
  // transition to ready instead.
  //
  // It is also subject to the one-ask-at-a-time rule above: if the opening ask
  // has not answered yet, that answer is already the one this step wants.
  const picking = state.step === "agent"
  React.useEffect(() => {
    if (!picking || gatewayStartupState.state !== "unmanaged") return
    ask()
  }, [picking, ask, gatewayStartupState.state])

  const keys = summonAccelerator(shortcuts)
  const platform = shortcutPlatform()

  // Two ways out that do not announce themselves, alongside the mark in the
  // corner that does — and only while there is something to leave. The browser
  // path keeps this hook mounted after setup finishes, so the listener is
  // attached and removed as setup comes and goes rather than staying on to eat
  // keys the panel and the browser should be getting.
  const active = isOnboarding(state)
  React.useEffect(
    () => listenForDismiss(window, active, () => setState(dismissOnboarding)),
    [active],
  )

  // While setup is teaching the summon shortcut, pressing it satisfies the
  // step: the keys light up and the way on appears. It does not move on by
  // itself, so the press is something a person sees land rather than a screen
  // that vanishes under them.
  const practising = state.step === "summon"
  // What the panel is doing right now, so the browser path can work out what
  // its own press would do to it — and so the cue is chosen before the state
  // changes rather than from inside an updater, which React is free to run more
  // than once.
  const showing = state.step === "summon" && state.summon === "shown"
  React.useEffect(() => {
    // Only where nothing else reports the press. A native host holds this
    // accelerator with the system and tells us about it, and counting both a
    // key and a report would make one press look like two.
    if (!practising || !keys || host.kind !== "browser") return
    function onKeyDown(event: KeyboardEvent) {
      if (event.repeat || !matchesAccelerator(event, keys)) return
      event.preventDefault()
      // There is no panel to toggle in a browser, so this is the nearest thing
      // to a report: the press flips whatever the surface last showed. It goes
      // through the same door as the host's own report, as the same kind of
      // fact, so the two paths cannot drift apart.
      const reported = !showing
      playCue(reported ? "summon" : "dismiss")
      setState((current) => pressSummon(current, reported))
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [practising, keys, showing])

  // The desktop host registers this accelerator with the system, so the press
  // is taken before this window sees a key — which is why it also reports the
  // summon it handled. That report is what makes the lesson work on the one
  // platform that ships: the panel really does appear and disappear, and the
  // keys light because it did.
  React.useEffect(() => {
    if (!practising) return
    let stop: (() => void) | undefined
    let cancelled = false
    // What the host reports is what the model records. It is the only thing
    // here that knows whether the panel is actually on screen, and a model that
    // counted presses instead could be told one thing by the sound, another by
    // the copy, and a third by the panel itself.
    void onSummoned((reported) => {
      playCue(reported ? "summon" : "dismiss")
      setState((current) => pressSummon(current, reported))
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
    active,
    accelerator: keys,
    platform,
    gatewayStartup: gatewayStartupState,
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
      if (!practising) return
      playCue("celebrate")
      setState(completeOnboarding)
      // Finishing is recorded in the state rather than persisted from here.
      // `completeOnboarding` keeps the agent and marks the ending a completion;
      // `dismissOnboarding` keeps nothing and marks it a dismissal. Which of
      // the two happened is half of what decides whether setup is written off
      // for good — the other half is whether the panel actually came up, which
      // this callback cannot know. The surface that learns it makes that call
      // (see `recordsSetupCompletion`).
    }, [practising]),
    // Leaving part-way is not an accomplishment and does not announce itself.
    // Leaving from the last step is a different act: the lesson is the final
    // thing setup has to say, so closing it there ends setup exactly as the way
    // on does, and it is heard to end. The two controls differ in what they
    // record, not in what they finish.
    dismiss: React.useCallback(() => {
      if (practising) playCue("celebrate")
      setState(dismissOnboarding)
    }, [practising]),
    recheck,
    retryGatewayStartup: retryStartup,
    checking,
  }
}
