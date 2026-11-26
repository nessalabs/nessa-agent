/**
 * The call's light: a soft glow that rises from behind the composer, in the
 * colour of the agent the chat runs (`data-agent`, `voice.css`), swelling
 * with whoever is talking. It lies behind the composer's dock and takes no
 * pointer. What each phase looks like is `model/rim.ts`'s table; this file
 * eases towards it once a frame and hands the numbers to the stylesheet as
 * custom properties, so nothing is drawn by script. With less motion it
 * holds each phase's still look (`stillRim`).
 */
import { useEffect, useRef } from "react"
import { reducedMotion } from "../../adapters/motion-preference"
import type { VoicePhase } from "../model/phase"
import { easeRim, rimLook, stillRim, type RimLook } from "../model/rim"

export function VoiceGlow({
  phase,
  microphoneOn,
  levels,
}: {
  phase: VoicePhase
  microphoneOn: boolean
  /** How loud the person and the voice are now, 0–1. */
  levels: () => { readonly input: number; readonly output: number }
}) {
  const ref = useRef<HTMLDivElement>(null)
  // What the frame loop reads: the newest props, without restarting the loop.
  const now = useRef({ phase, microphoneOn, levels })
  now.current = { phase, microphoneOn, levels }

  useEffect(() => {
    const glow = ref.current
    if (!glow) return
    let frame = 0
    let last = performance.now()
    let shown: RimLook = rimLook("idle", { input: 0, output: 0 }, true)
    const tick = (time: number) => {
      const seconds = Math.min(0.1, (time - last) / 1000)
      last = time
      const { phase, microphoneOn, levels } = now.current
      shown = reducedMotion()
        ? stillRim(phase, microphoneOn)
        : easeRim(shown, rimLook(phase, levels(), microphoneOn), seconds)
      // A circling light has no place in a glow from below: it shows as the
      // glow breathing, slowly, at the light's strength.
      const breath = shown.sweep * (0.5 + 0.5 * Math.sin(time / 600))
      const strength = Math.min(1, shown.glow * 1.4 + shown.level * 0.5 + breath * 0.25)
      glow.style.setProperty("--voice-glow", strength.toFixed(3))
      glow.style.setProperty("--voice-reach", (1 + shown.level * 0.3).toFixed(3))
      glow.style.setProperty("--voice-grey", shown.grey.toFixed(3))
      frame = requestAnimationFrame(tick)
    }
    frame = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(frame)
  }, [])

  return <div ref={ref} className="voice-glow" aria-hidden="true" />
}
