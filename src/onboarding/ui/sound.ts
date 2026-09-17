import warmDissolve from "./warm-dissolve.mp3"
import celebrationSound from "./sounds/celebration.m4a"
import selectSound from "./sounds/select.m4a"
import toggleOffSound from "./sounds/toggle-off.m4a"
import toggleOnSound from "./sounds/toggle-on.m4a"

/**
 * The moments in setup that make a sound, named for the moment rather than for
 * the file — what a step means is stable, which sound says it is not.
 */
export type Cue =
  /** The window opening. */
  | "intro"
  /** An agent picked out of the list. */
  | "choose"
  /** A step accepted, and the next one arriving. */
  | "advance"
  /** The summon shortcut putting Nessa on screen. */
  | "summon"
  /** The same shortcut putting it away again. */
  | "dismiss"
  /** Setup finished. */
  | "celebrate"

/**
 * Where each cue's sound comes from, and how loud it sits.
 *
 * The interface sounds are SND01 "sine" (snd.dev, Yasuhiro Tsuchiya), used
 * under its terms: free for commercial use in a product, with only
 * redistribution of the pack itself prohibited. They are cut out of that kit's
 * sprite rather than fetched from its CDN, because an app that goes quiet
 * without a network connection is worse than one with no sound at all.
 *
 * The levels are deliberately low. These are punctuation for something the
 * person is already doing and looking at; a UI sound at the volume of media is
 * startling the first time and intolerable the tenth.
 */
const CUES: Readonly<Record<Cue, { src: string; volume: number }>> = Object.freeze({
  intro: { src: warmDissolve, volume: 0.35 },
  // Choosing an agent is a thing being switched on, and it is the kit's toggle
  // that says so — the same sound the summon lesson uses, because it is the
  // same act.
  choose: { src: toggleOnSound, volume: 0.32 },
  advance: { src: selectSound, volume: 0.3 },
  summon: { src: toggleOnSound, volume: 0.34 },
  dismiss: { src: toggleOffSound, volume: 0.34 },
  celebrate: { src: celebrationSound, volume: 0.4 },
})

/**
 * Sound is part of the opening's motion, so a reduced-motion preference takes
 * it with the rest. It is also the only control anyone has over it here —
 * there is no setting yet — which is a reason to respect it exactly.
 */
function silenced() {
  if (typeof window === "undefined") return true
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches
}

/** One element per cue, kept so a sound is decoded once rather than on every
 * press. Setup's cues are short and never overlap themselves. */
const players = new Map<Cue, HTMLAudioElement>()

function player(cue: Cue): HTMLAudioElement {
  const existing = players.get(cue)
  if (existing) return existing
  const { src, volume } = CUES[cue]
  const audio = new Audio(src)
  audio.volume = volume
  players.set(cue, audio)
  return audio
}

/**
 * Play a cue, from its start.
 *
 * A host that blocks playback without a gesture rejects this, and the
 * rejection is swallowed: nothing setup does should wait on, or fail for, a
 * sound. Replaying from zero matters for the toggle, where the same cue can
 * land twice in quick succession.
 */
export function playCue(cue: Cue): void {
  if (silenced()) return
  const audio = player(cue)
  audio.currentTime = 0
  void audio.play().catch(() => {})
}

/** Stop a cue that is still sounding, for when its moment is abandoned. */
export function stopCue(cue: Cue): void {
  players.get(cue)?.pause()
}
