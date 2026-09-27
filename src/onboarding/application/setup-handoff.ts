/**
 * What the setup surface is doing about the handoff, as a value.
 *
 * The surface has to decide four things while setup is ending: whether it is
 * time to hand over, what an answer means, what is left to say when the handoff
 * did not end with this window gone, and what a retry puts back. None of those
 * are rendering, and none of them need a window to be decided — but all of them
 * used to be decided inside effects in the component that draws the screen,
 * where the only way to reach them was to have a window and destroy it.
 *
 * So they live here, for the reason `setup-recovery.ts` and `readiness-check.ts`
 * do: the decision is the part worth testing. `use-setup-handoff.ts` is the thin
 * layer that performs the host calls this asks for, and `setup-gate.tsx` draws
 * what it reports.
 */

import type { SetupHandoff, SetupWindowClose } from "../../host"

/** Where the handoff has got to. */
export interface SetupHandoffState {
  /**
   * Which attempt is outstanding, or `undefined` when none is. Retrying counts
   * up, and an answer names the attempt it belongs to, so the answer to an
   * abandoned attempt cannot be mistaken for the answer to the current one.
   * While an attempt is outstanding this is also what keeps a re-render from
   * asking a second time.
   */
  requested: number | undefined
  /** The answer, or `undefined` while there is none. */
  outcome: SetupHandoff | undefined
  /** True once a close was asked for and the window system did not do it. */
  closeFailed: boolean
  /** How many attempts have been made, so the next one has its own number. */
  attempts: number
  /**
   * Which host call the next attempt makes.
   *
   * The two are not interchangeable. `hand-over` shows the panel, writes setup
   * off and closes this window; `record` does the last two only, and is what
   * the screen after a refused write asks for — the panel is already up, and
   * showing it again re-anchors and refits a window somebody may have moved to.
   */
  resume: "hand-over" | "record"
}

/** Nothing asked for, nothing answered. */
export const noSetupHandoff: SetupHandoffState = {
  requested: undefined,
  outcome: undefined,
  closeFailed: false,
  attempts: 0,
  resume: "hand-over",
}

/** What can happen to the handoff. */
export type SetupHandoffEvent =
  /** The handoff has just been asked for. */
  | { type: "requested" }
  /** The host answered attempt `attempt`, or that call rejected. */
  | { type: "settled"; attempt: number; outcome: SetupHandoff }
  /** Somebody pressed the retry on the recovery screen. */
  | { type: "retry" }
  /** Somebody pressed "save again" on the screen a refused write left. */
  | { type: "save-again" }
  /** A close was asked for and the host said what it did. */
  | { type: "closed"; close: SetupWindowClose }

/**
 * The state one event leaves.
 *
 * An answer carries the attempt it belongs to, and only the outstanding
 * attempt's answer is applied. Today a retry is only offered once an answer is
 * already on screen, so nothing is in flight when one is made; the number is
 * what keeps that from being load-bearing. `finishSetupWindow` cannot be
 * cancelled, so any call that does outlive its attempt answers to a number
 * nothing is waiting on rather than to whatever is outstanding now.
 */
export function applySetupHandoff(
  state: SetupHandoffState,
  event: SetupHandoffEvent,
): SetupHandoffState {
  switch (event.type) {
    case "requested":
      return state.requested === undefined
        ? { ...state, requested: state.attempts + 1, attempts: state.attempts + 1 }
        : state
    case "settled":
      return state.requested === event.attempt
        ? { ...state, requested: undefined, outcome: event.outcome }
        : state
    case "retry":
      // Everything the previous attempt left, except how many there have been:
      // the next attempt needs a number the outstanding one cannot answer to.
      return { ...noSetupHandoff, attempts: state.attempts }
    case "save-again":
      // The same fresh attempt, pointed at the write alone. Reached only from
      // the screen a refused write leaves, where the panel is already up.
      return { ...noSetupHandoff, attempts: state.attempts, resume: "record" }
    case "closed":
      // A browser has no window of its own to close. That is not a failure, and
      // it is also not a screen this can be reached from — the old surface
      // treated it as one, which no screen ever saw.
      return { ...state, closeFailed: event.close.outcome === "close-failed" }
  }
}

/**
 * Whether the handoff should be asked for now.
 *
 * Only once setup is over, and only while no attempt is outstanding and none has
 * answered — a settled attempt is not asked again except through a retry.
 */
export function shouldHandOver(state: SetupHandoffState, setupActive: boolean): boolean {
  return !setupActive && state.requested === undefined && state.outcome === undefined
}

/**
 * Lets one thing through at a time.
 *
 * The handoff is asked for from an effect, which reads its own copy of the
 * state: two runs against the same copy — a development double-invoke, or a
 * re-render arriving before the dispatch does — both see the same answer and
 * both would call. The reducer absorbs the second `requested`; the host call is
 * the one that has to be stopped, because `finishSetupWindow` shows the panel,
 * writes setup off for good and closes the window.
 *
 * It is released when the call it was claimed for settles, and at no other time.
 * A retry does not release it: if one could ever arrive while a call is still
 * outstanding, letting the next attempt start would mean two of those calls at
 * once, which is the thing being prevented. The attempt number is what keeps the
 * abandoned call's answer from being read as the new one's.
 */
export interface HandoffGate {
  /** True if the caller may proceed; false if something already holds it. */
  claim(): boolean
  /** Give it back, so the next attempt can claim it. */
  release(): void
}

/** A gate holding nothing. */
export function createHandoffGate(): HandoffGate {
  let held = false
  return {
    claim: () => {
      if (held) return false
      held = true
      return true
    },
    release: () => {
      held = false
    },
  }
}

/** A rejected handoff call is a panel that did not come up, with its cause. */
export function handoffRejection(cause: unknown): SetupHandoff {
  return { outcome: "panel-unavailable", cause }
}
