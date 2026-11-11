/**
 * How the glow on the composer's edge looks for a call's phase: the numbers
 * the rim's drawing eases towards (`ui/voice-rim.tsx`). Kept apart from the
 * drawing so what each phase looks like is one table, tested without a
 * canvas.
 *
 * The rim is quiet by design: a thin line of light with a short halo, never
 * a wash over the conversation.
 */
import type { VoicePhase } from "../application/voice-controller"

export interface RimLook {
  /** How bright the rim and its halo are, 0–1. */
  readonly glow: number
  /** How far the halo reaches and how lively it is, 0–1: the loudness. */
  readonly level: number
  /** How strongly one light circles the edge, 0–1. */
  readonly sweep: number
  /** How fast that light circles, in radians a second. */
  readonly spin: number
  /** 0 is the person's sky blue, 1 the voice's periwinkle. */
  readonly hue: number
  /** 0 keeps the colour, 1 drains it: a microphone that is off. */
  readonly grey: number
}

const dark: RimLook = { glow: 0, level: 0, sweep: 0, spin: 0, hue: 0, grey: 1 }

/**
 * The look for `phase`. While listening the rim follows the person's voice
 * (`input`); while the voice speaks it follows that (`output`) and turns
 * periwinkle. Connecting and working are one circling light, working's
 * slower and dimmer. With the microphone off, listening is a faint grey.
 */
export function rimLook(
  phase: VoicePhase,
  levels: { readonly input: number; readonly output: number },
  microphoneOn: boolean,
): RimLook {
  switch (phase) {
    case "connecting":
      return { glow: 0.08, level: 0.1, sweep: 1, spin: 2.2, hue: 0, grey: 0 }
    case "listening": {
      if (!microphoneOn) return { ...dark, glow: 0.08, level: 0.05 }
      const heard = clamp(levels.input)
      return {
        glow: 0.2 + 0.3 * heard,
        level: Math.max(0.1, heard),
        sweep: 0,
        spin: 0,
        hue: 0,
        grey: 0,
      }
    }
    case "speaking":
      return {
        glow: 0.5,
        level: clamp(levels.output),
        sweep: 0,
        spin: 0,
        hue: 1,
        grey: 0,
      }
    case "working":
      return { glow: 0.16, level: 0.12, sweep: 0.45, spin: 0.9, hue: 0.5, grey: 0 }
    case "idle":
    case "failed":
    case "dropped":
      return dark
  }
}

/**
 * One step from `shown` towards `wanted` after `seconds`: everything eases at
 * one unhurried pace, and loudness a little quicker, so a word swells the
 * rim without flickering it.
 */
export function easeRim(shown: RimLook, wanted: RimLook, seconds: number): RimLook {
  const slow = Math.min(1, seconds * 6)
  const quick = Math.min(1, seconds * 5)
  const toward = (from: number, to: number, share: number) => from + (to - from) * share
  return {
    glow: toward(shown.glow, wanted.glow, slow),
    level: toward(shown.level, wanted.level, quick),
    sweep: toward(shown.sweep, wanted.sweep, slow),
    // A speed is taken at once: easing it would run the light backwards.
    spin: wanted.spin,
    hue: toward(shown.hue, wanted.hue, slow),
    grey: toward(shown.grey, wanted.grey, slow),
  }
}

/** The look with nothing moving, for less motion: no circling light, a level that holds. */
export function stillRim(look: RimLook): RimLook {
  return {
    ...look,
    glow: Math.max(look.glow, look.sweep > 0 ? 0.2 : 0),
    level: 0.2,
    sweep: 0,
    spin: 0,
  }
}

function clamp(value: number): number {
  return Number.isFinite(value) ? Math.min(1, Math.max(0, value)) : 0
}
