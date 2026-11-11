/**
 * The call pill's rules: when the pill shows, what it draws, and what each
 * press on it means for the call. The pill is a second small window that
 * floats over other apps (`pill/`); it holds no call of its own.
 *
 * It shows only while a call is open and this window is not the one in
 * front: in front, the composer already carries the same switches.
 */
import type { VoicePhase } from "./phase"

/** The phases of an open call: the ones a pill is drawn for. */
export type OpenPhase = "connecting" | "listening" | "working" | "speaking"

/** The call as the pill draws it. */
export interface CallPicture {
  readonly phase: OpenPhase
  /** The chat the call talks to, by its title. */
  readonly title: string
  readonly microphoneOn: boolean
  readonly speakerOn: boolean
}

/** What the person pressed on the pill. `open`: the chat's name. */
export type CallPress = "end" | "mute" | "unmute" | "speaker-off" | "speaker-on" | "open"

/** What a press asks of the call. `show` is the chat brought forward. */
export type CallPressed =
  | { readonly kind: "end" }
  | { readonly kind: "microphone"; readonly on: boolean }
  | { readonly kind: "speaker"; readonly on: boolean }
  | { readonly kind: "show" }

function openPhase(phase: VoicePhase): OpenPhase | null {
  switch (phase) {
    case "connecting":
    case "listening":
    case "working":
    case "speaking":
      return phase
    case "idle":
    case "failed":
    case "dropped":
      return null
  }
}

/**
 * What the pill draws for a call, or null when it does not show: no call is
 * open, or this window is in front.
 */
export function callPicture(
  call: {
    readonly phase: VoicePhase
    readonly microphoneOn: boolean
    readonly speakerOn: boolean
  },
  title: string,
  windowInFront: boolean,
): CallPicture | null {
  const phase = openPhase(call.phase)
  if (phase === null || windowInFront) return null
  return {
    phase,
    title: title.trim() || "Voice call",
    microphoneOn: call.microphoneOn,
    speakerOn: call.speakerOn,
  }
}

/** Whether two pictures draw the same pill. */
export function samePicture(a: CallPicture | null, b: CallPicture | null): boolean {
  if (a === null || b === null) return a === b
  return (
    a.phase === b.phase &&
    a.title === b.title &&
    a.microphoneOn === b.microphoneOn &&
    a.speakerOn === b.speakerOn
  )
}

export function callPressed(press: CallPress): CallPressed {
  switch (press) {
    case "end":
      return { kind: "end" }
    case "mute":
      return { kind: "microphone", on: false }
    case "unmute":
      return { kind: "microphone", on: true }
    case "speaker-off":
      return { kind: "speaker", on: false }
    case "speaker-on":
      return { kind: "speaker", on: true }
    case "open":
      return { kind: "show" }
  }
}

/** The pill's one line of words for a picture. */
export function pillWords(picture: CallPicture): string {
  if (!picture.microphoneOn && picture.phase === "listening") return "Muted"
  switch (picture.phase) {
    case "connecting":
      return "Connecting"
    case "listening":
      return "Listening"
    case "working":
      return "Working"
    case "speaking":
      return "Speaking"
  }
}
