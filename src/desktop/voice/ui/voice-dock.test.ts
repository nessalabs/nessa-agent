import { expect, it } from "vitest"
import type { VoiceSnapshot } from "../application/voice-controller"
import { voiceLine, voiceLineRuns } from "./voice-dock"

const call: VoiceSnapshot = {
  phase: "listening",
  sessionId: "s1",
  hearing: "",
  saying: "",
  microphoneOn: true,
  speakerOn: true,
  notice: null,
  hint: null,
  fix: null,
}

it("runs on one line only for words said or heard, never for where the call stands", () => {
  const runs = (change: Partial<VoiceSnapshot>) => voiceLineRuns({ ...call, ...change })
  expect(runs({})).toBe(false)
  expect(runs({ hint: "No sound is coming from the microphone." })).toBe(false)
  expect(runs({ hearing: "please look at" })).toBe(true)
  expect(runs({ phase: "working", hearing: "and also" })).toBe(true)
  expect(runs({ phase: "working" })).toBe(false)
  expect(runs({ phase: "speaking", saying: "I can pass that along." })).toBe(true)
  expect(runs({ phase: "speaking", saying: "  " })).toBe(false)
  expect(runs({ phase: "connecting", hearing: "x" })).toBe(false)
  expect(runs({ phase: "dropped", notice: "The call dropped." })).toBe(false)
  // What runs is always the words themselves.
  expect(voiceLine({ ...call, phase: "speaking", saying: " Hello. " })).toBe("Hello.")
})
