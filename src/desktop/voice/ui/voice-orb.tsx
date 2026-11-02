/**
 * The call's orb: a lit sphere in the colours of the agent the chat runs,
 * drawn like the planet on its agent card. It breathes while it listens,
 * swells with the person's voice and the agent's, and turns its light slowly
 * while the agent works. The loudness is read each frame from the call
 * (`levels`) into one custom property; the rest is CSS. Its colours are
 * the design system's agent card tones (`agentCardTones`), not its own.
 */
import { useEffect, useRef, type CSSProperties } from "react"
import { agentCardTones } from "@nessa-ui/react/agent-card"
import type { VoicePhase } from "../application/voice-controller"
import "./voice.css"

/** The agent card tone an agent's orb wears; any other agent wears `local`. */
export type OrbTone = keyof typeof agentCardTones

export function orbTone(agent: string | undefined): OrbTone {
  return agent === "claude" || agent === "codex" ? agent : "local"
}

export function VoiceOrb({
  phase,
  tone,
  levels,
  size = 88,
}: {
  phase: VoicePhase
  tone: OrbTone
  /** How loud the person and the voice are now, 0–1. */
  levels: () => { readonly input: number; readonly output: number }
  size?: number
}) {
  const ref = useRef<HTMLDivElement>(null)
  const colours = agentCardTones[tone]
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
      data-tone={tone}
      style={
        {
          width: size,
          height: size,
          "--orb-lit": colours.lit,
          "--orb-planet": colours.top,
          "--orb-deep": colours.bottom,
          "--orb-glow": colours.halo,
        } as CSSProperties
      }
      aria-hidden
    >
      <span className="voice-orb-light" />
    </div>
  )
}
