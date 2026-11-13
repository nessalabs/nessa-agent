/**
 * What crosses between the desktop window and the call pill: the call as the
 * pill draws it, and what the person pressed there. The shapes are declared
 * again in `src-tauri/src/call_pill.rs`, which reads the same JSON; whatever
 * arrives is read here before anything acts on it, whichever page sent it.
 */

export const CALL_PILL_PHASES = [
  "connecting",
  "listening",
  "working",
  "speaking",
] as const

/** Where an open call stands. A call that is not open has no picture. */
export type CallPillPhase = (typeof CALL_PILL_PHASES)[number]

/** The call as the pill draws it. */
export interface CallPicture {
  readonly phase: CallPillPhase
  /** The chat the call talks to, by its title. */
  readonly title: string
  readonly microphoneOn: boolean
  readonly speakerOn: boolean
}

export const CALL_PRESSES = [
  "end",
  "mute",
  "unmute",
  "speaker-off",
  "speaker-on",
  "open",
] as const

/** What the person pressed on the pill. `open`: the chat's name. */
export type CallPress = (typeof CALL_PRESSES)[number]

/** `value` as a picture, or null when it is not one. */
export function parseCallPicture(value: unknown): CallPicture | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null
  const { phase, title, microphoneOn, speakerOn } = value as Record<string, unknown>
  if (
    typeof phase !== "string" ||
    !(CALL_PILL_PHASES as readonly string[]).includes(phase) ||
    typeof title !== "string" ||
    typeof microphoneOn !== "boolean" ||
    typeof speakerOn !== "boolean"
  )
    return null
  return { phase: phase as CallPillPhase, title, microphoneOn, speakerOn }
}

/** `value` as a press, or null when it is not one. */
export function parseCallPress(value: unknown): CallPress | null {
  return typeof value === "string" && (CALL_PRESSES as readonly string[]).includes(value)
    ? (value as CallPress)
    : null
}
