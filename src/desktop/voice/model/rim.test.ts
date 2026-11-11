import { describe, expect, it } from "vitest"
import { easeRim, rimLook, stillRim } from "./rim"

const quiet = { input: 0, output: 0 }

describe("rimLook", () => {
  it("circles one light while connecting and, slower and dimmer, while working", () => {
    const connecting = rimLook("connecting", quiet, true)
    const working = rimLook("working", quiet, true)
    expect(connecting.sweep).toBe(1)
    expect(working.sweep).toBeGreaterThan(0)
    expect(working.sweep).toBeLessThan(connecting.sweep)
    expect(working.spin).toBeLessThan(connecting.spin)
  })

  it("follows the person while listening and the voice while it speaks", () => {
    const silent = rimLook("listening", quiet, true)
    const talking = rimLook("listening", { input: 0.8, output: 0 }, true)
    expect(talking.glow).toBeGreaterThan(silent.glow)
    expect(talking.level).toBe(0.8)
    expect(talking.hue).toBe(0)
    const speaking = rimLook("speaking", { input: 0.9, output: 0.4 }, true)
    expect(speaking.level).toBe(0.4)
    expect(speaking.hue).toBe(1)
  })

  it("stays subtle: never brighter than half, however loud", () => {
    for (const phase of ["connecting", "listening", "speaking", "working"] as const)
      expect(rimLook(phase, { input: 5, output: 5 }, true).glow).toBeLessThanOrEqual(0.5)
  })

  it("is a faint grey with the microphone off", () => {
    const off = rimLook("listening", { input: 0.9, output: 0 }, false)
    expect(off.grey).toBe(1)
    expect(off.glow).toBeLessThan(rimLook("listening", quiet, true).glow)
  })

  it("is dark with no call, and takes a level that is not a number as silence", () => {
    for (const phase of ["idle", "failed", "dropped"] as const)
      expect(rimLook(phase, { input: 1, output: 1 }, true).glow).toBe(0)
    expect(rimLook("speaking", { input: 0, output: Number.NaN }, true).level).toBe(0)
  })
})

describe("easeRim", () => {
  it("moves part of the way in a frame and all of it in a long step", () => {
    const from = rimLook("idle", quiet, true)
    const to = rimLook("speaking", { input: 0, output: 1 }, true)
    const frame = easeRim(from, to, 1 / 60)
    expect(frame.glow).toBeGreaterThan(from.glow)
    expect(frame.glow).toBeLessThan(to.glow)
    expect(easeRim(from, to, 1)).toEqual(to)
  })
})

describe("stillRim", () => {
  it("holds a connecting call as a steady rim with nothing circling", () => {
    const still = stillRim("connecting", true)
    expect(still.sweep).toBe(0)
    expect(still.spin).toBe(0)
    expect(still.glow).toBeGreaterThan(0)
  })

  it("takes no loudness, so nothing a voice does can move it", () => {
    // The moving look does follow the voice in these phases: the still one
    // is not given the levels at all.
    expect(rimLook("listening", { input: 1, output: 0 }, true)).not.toEqual(
      rimLook("listening", { input: 0, output: 0 }, true),
    )
    for (const phase of ["listening", "speaking", "working"] as const) {
      const still = stillRim(phase, true)
      expect(still.glow).toBeGreaterThan(0)
      expect(still.sweep).toBe(0)
      expect(still).toEqual(stillRim(phase, true))
    }
  })

  it("keeps a muted microphone grey and a call that is over dark", () => {
    expect(stillRim("listening", false).grey).toBe(1)
    expect(stillRim("idle", true).glow).toBe(0)
    expect(stillRim("idle", true).level).toBe(0)
  })
})
