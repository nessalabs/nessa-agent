import curiousSilence from "./curious-silence.mp3"
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
  intro: { src: curiousSilence, volume: 0.35 },
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
 * Whether setup's sounds are off.
 *
 * A reduced-motion preference is where this *starts*, because someone who has
 * asked for less movement is unlikely to want an unprompted chime either. It is
 * not where it stays: motion and sound are two preferences, and tying them
 * together left anyone who wanted one without the other — or who is listening
 * to a screen reader the clip talks over — with no control at all. The toggle
 * in setup's chrome moves it in both directions.
 *
 * Session-local on purpose. Nessa has no settings store yet, and inventing one
 * for a single switch on a screen seen once would be a worse answer than a
 * switch that works for as long as the screen is up.
 */
let muted = quietByDefault()
const listeners = new Set<() => void>()

function quietByDefault(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function")
    return true
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches
}

/** Whether cues are currently silenced. */
export function soundMuted(): boolean {
  return muted
}

/** Turn setup's cues on or off. Silencing also stops whatever is sounding:
 * a mute that lets the current clip finish is not a mute. */
export function setSoundMuted(next: boolean): void {
  if (muted === next) return
  muted = next
  if (next) for (const audio of players.values()) audio.pause()
  for (const listener of listeners) listener()
}

/** Subscribe to the preference changing, for a control that shows its state. */
export function subscribeSoundMuted(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

function silenced() {
  return typeof window === "undefined" || muted
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
