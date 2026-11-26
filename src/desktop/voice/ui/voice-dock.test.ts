import { expect, it } from "vitest"
import type { VoiceSnapshot } from "../application/voice-controller"
import { voiceLines, voiceStatus } from "./voice-dock"

const call: VoiceSnapshot = {
  phase: "listening",
  sessionId: "s1",
  hearing: "",
  saying: "",
  transcript: [],
  microphoneOn: true,
  speakerOn: true,
  notice: null,
  hint: null,
  fix: null,
}

it("says where the call stands in a word or two, and why a call stopped", () => {
  const status = (change: Partial<VoiceSnapshot>, agent?: string) =>
    voiceStatus({ ...call, ...change }, agent)
  expect(status({ phase: "connecting" })).toBe("Connecting…")
  expect(status({})).toBe("Listening")
  // Words being heard are the lines', not the header's.
  expect(status({ hearing: "please look at" })).toBe("Listening")
  expect(status({ microphoneOn: false })).toBe("Microphone off")
  expect(status({ phase: "working" }, "Codex")).toBe("Codex is working…")
  expect(status({ phase: "working" })).toBe("Working…")
  expect(status({ phase: "speaking", saying: "Sure." })).toBe("Speaking")
  expect(status({ phase: "paused" })).toBe("Paused")
  expect(status({ phase: "dropped", notice: "The call dropped." })).toBe(
    "The call dropped.",
  )
  expect(status({ phase: "idle" })).toBe("")
})

it("lists what was written down, then the words still being said and heard", () => {
  const transcript = [
    { who: "person", text: "Push the fix." },
    { who: "voice", text: "Sent." },
  ] as const
  expect(voiceLines({ ...call, transcript })).toEqual([
    { who: "person", text: "Push the fix.", live: false },
    { who: "voice", text: "Sent.", live: false },
  ])
  expect(
    voiceLines({
      ...call,
      phase: "speaking",
      transcript,
      saying: " The PR is ",
      hearing: "and also ",
    }).slice(2),
  ).toEqual([
    { who: "voice", text: "The PR is", live: true },
    { who: "person", text: "and also", live: true },
  ])
  // What the voice said is the transcript's once it is done: no second copy.
  expect(voiceLines({ ...call, transcript, saying: "Sent." })).toHaveLength(2)
  expect(voiceLines({ ...call, phase: "speaking", saying: "  " })).toEqual([])
})
