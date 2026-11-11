import { expect, it } from "vitest"
import { CALL_PRESSES, parseCallPicture, parseCallPress } from "./call-pill"

const picture = {
  phase: "listening",
  title: "Fix the build",
  microphoneOn: true,
  speakerOn: false,
}

it("reads a picture, and keeps only what a picture holds", () => {
  expect(parseCallPicture(picture)).toEqual(picture)
  expect(parseCallPicture({ ...picture, extra: 1 })).toEqual(picture)
})

it("refuses what is not a picture", () => {
  for (const value of [
    null,
    undefined,
    "listening",
    [picture],
    { ...picture, phase: "idle" },
    { ...picture, phase: "failed" },
    { ...picture, title: 3 },
    { ...picture, microphoneOn: "yes" },
    { phase: "listening", title: "" },
  ])
    expect(parseCallPicture(value)).toBeNull()
})

it("reads each press by its name, and nothing else", () => {
  for (const press of CALL_PRESSES) expect(parseCallPress(press)).toBe(press)
  for (const value of ["quit", "", "End", 1, null, { press: "end" }])
    expect(parseCallPress(value)).toBeNull()
})
