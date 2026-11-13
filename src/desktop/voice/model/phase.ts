/**
 * Where a call stands. `working`: the agent is on a request the voice
 * sent. `speaking`: the voice is answering. `failed`: no call started.
 * `dropped`: an open call ended on its own.
 */
export type VoicePhase =
  "idle" | "connecting" | "listening" | "working" | "speaking" | "failed" | "dropped"
