/**
 * The call's orb: a small glossy sky, blue at the top and pale toward the
 * bottom, with soft clouds drifting across it. It is the same for every
 * agent: the orb is Nessa's voice, not the agent's. It breathes while it
 * connects and listens, swells with the person's voice and the agent's, and
 * pulses quicker while the agent works. The loudness is read each
 * frame from the call (`levels`) into one custom property; the rest is CSS.
 */
import { useEffect, useRef, type CSSProperties } from "react"
import type { VoicePhase } from "../application/voice-controller"
import "./voice.css"

export function VoiceOrb({
  phase,
  levels,
  size = 88,
}: {
  phase: VoicePhase
  /** How loud the person and the voice are now, 0–1. */
  levels: () => { readonly input: number; readonly output: number }
  size?: number
}) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const orb = ref.current
    if (!orb) return
    let frame = 0
    let shown = 0
    const tick = () => {
      const { input, output } = levels()
      // Quick to rise, slow to fall, so a word does not flicker it.
      const level = Math.max(input, output)
      shown = level > shown ? level : shown * 0.9 + level * 0.1
      orb.style.setProperty("--voice-level", shown.toFixed(3))
      frame = requestAnimationFrame(tick)
    }
    frame = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(frame)
  }, [levels])
  return (
    <div
      ref={ref}
      className="voice-orb"
      data-phase={phase}
      style={{ "--voice-orb-size": `${size}px` } as CSSProperties}
      aria-hidden
    >
      <span className="voice-orb-sky">
        <span className="voice-orb-clouds" />
      </span>
      <span className="voice-orb-sky" data-layer="far">
        <span className="voice-orb-clouds" />
      </span>
      <span className="voice-orb-gloss" />
    </div>
  )
}
