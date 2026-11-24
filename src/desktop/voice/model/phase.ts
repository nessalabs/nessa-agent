/**
 * Where a call stands. `working`: the agent is on a request the voice
 * sent. `speaking`: the voice is answering. `failed`: no call started.
 * `dropped`: an open call ended on its own. `paused`: the person paused an
 * open call: it is hung up with the provider, so nothing is billed, and is
 * kept, with what was said in it, to resume.
 */
export type VoicePhase =
  | "idle"
  | "connecting"
  | "listening"
  | "working"
  | "speaking"
  | "paused"
  | "failed"
  | "dropped"
