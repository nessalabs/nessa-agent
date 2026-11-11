import { expect, it } from "vitest"
import { callPicture, callPressed, pillWords, samePicture } from "./call-pill"

const call = { phase: "listening", microphoneOn: true, speakerOn: true } as const

it("draws a pill only for an open call in a window that is not in front", () => {
  expect(callPicture(call, "Fix the build", false)).toEqual({
    phase: "listening",
    title: "Fix the build",
    microphoneOn: true,
    speakerOn: true,
  })
  expect(callPicture(call, "Fix the build", true)).toBeNull()
  for (const phase of ["idle", "failed", "dropped"] as const)
    expect(callPicture({ ...call, phase }, "Fix the build", false)).toBeNull()
  for (const phase of ["connecting", "working", "speaking"] as const)
    expect(callPicture({ ...call, phase }, "Fix the build", false)?.phase).toBe(phase)
})

it("names a chat that has no title yet", () => {
  expect(callPicture(call, "  ", false)?.title).toBe("Voice call")
})

it("tells two pictures apart by anything the pill draws", () => {
  const picture = callPicture(call, "A", false)
  expect(samePicture(picture, callPicture(call, "A", false))).toBe(true)
  expect(samePicture(picture, callPicture(call, "B", false))).toBe(false)
  expect(
    samePicture(picture, callPicture({ ...call, speakerOn: false }, "A", false)),
  ).toBe(false)
  expect(samePicture(picture, null)).toBe(false)
  expect(samePicture(null, null)).toBe(true)
})

it("turns each press into what it asks of the call", () => {
  expect(callPressed("end")).toEqual({ kind: "end" })
  expect(callPressed("mute")).toEqual({ kind: "microphone", on: false })
  expect(callPressed("unmute")).toEqual({ kind: "microphone", on: true })
  expect(callPressed("speaker-off")).toEqual({ kind: "speaker", on: false })
  expect(callPressed("speaker-on")).toEqual({ kind: "speaker", on: true })
  expect(callPressed("open")).toEqual({ kind: "show" })
})

it("says Muted for a listening call whose microphone is off, and the phase otherwise", () => {
  const picture = callPicture(call, "A", false)
  if (!picture) throw new Error("no picture")
  expect(pillWords(picture)).toBe("Listening")
  expect(pillWords({ ...picture, microphoneOn: false })).toBe("Muted")
  expect(pillWords({ ...picture, phase: "speaking", microphoneOn: false })).toBe(
    "Speaking",
  )
  expect(pillWords({ ...picture, phase: "connecting" })).toBe("Connecting")
  expect(pillWords({ ...picture, phase: "working" })).toBe("Working")
})
