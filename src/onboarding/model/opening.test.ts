import { describe, expect, it } from "vitest"
import {
  FLIGHT_LIGHT_PX,
  OPENING_BEATS,
  beatLength,
  coverScale,
  flightFrames,
  flightTarget,
  nextBeat,
  openingBeat,
  setupField,
} from "./opening"

describe("the opening's beats", () => {
  it("runs glow, fill, form, open on one clock", () => {
    expect(openingBeat(0)).toBe("glow")
    expect(openingBeat(999)).toBe("glow")
    expect(openingBeat(1000)).toBe("fill")
    expect(openingBeat(2000)).toBe("form")
    expect(openingBeat(2900)).toBe("open")
    expect(openingBeat(60_000)).toBe("open")
  })

  it("walks each beat to the next after its own length, and stops at open", () => {
    let beat = OPENING_BEATS[0].beat
    let at = 0
    while (beatLength(beat) !== undefined) {
      at += beatLength(beat)!
      beat = nextBeat(beat)
      expect(openingBeat(at)).toBe(beat)
    }
    expect(beat).toBe("open")
    expect(nextBeat("open")).toBe("open")
  })
})

describe("the window's light", () => {
  const at = setupField

  it("is a see-through glow until it forms, then the window's own ground", () => {
    expect(at("glow").clear).toBe(1)
    expect(at("fill").clear).toBe(1)
    expect(at("form").clear).toMatchObject({ to: 0 })
    expect(at("open").clear).toBe(0)
  })

  it("floods past the edges before it draws in to the window", () => {
    expect(at("fill").spread).toMatchObject({ to: 1.25 })
    expect(at("form").spread).toMatchObject({ to: 1 })
  })

  it("sits to the right once open, behind the picture", () => {
    expect(at("open").x).toMatchObject({ to: 0.72 })
  })
})

describe("covering the screen while it is a glow", () => {
  it("is large enough that the rounded corners fall off the screen", () => {
    const cover = coverScale({ width: 1470, height: 956 }, { width: 820, height: 540 })
    expect(820 * cover).toBeGreaterThan(1470)
    expect(540 * cover).toBeGreaterThan(956)
  })

  it("never shrinks the window, and survives an unmeasured one", () => {
    expect(
      coverScale({ width: 100, height: 100 }, { width: 820, height: 540 }),
    ).toBeGreaterThanOrEqual(1)
    expect(coverScale({ width: 1470, height: 956 }, { width: 0, height: 0 })).toBe(1)
  })
})

describe("flying into the menu bar", () => {
  const window = { left: 325, top: 208, width: 820, height: 540 }

  it("gathers to an icon-sized circle in place, then lands on the tray icon", () => {
    const frames = flightFrames(window, { x: 1400, y: 11 })
    const gathered = frames.find((frame) => frame.offset === 0.45)!
    expect(gathered.transform).toBe(
      `scale(${FLIGHT_LIGHT_PX / 820}, ${FLIGHT_LIGHT_PX / 540})`,
    )
    expect(gathered.borderRadius).toBe("50%")
    // From the window's centre (735, 478) to the icon.
    expect(frames.at(-1)!.transform).toContain("translate(665px, -467px)")
    expect(frames.at(-1)!.opacity).toBe(0)
  })

  it("aims at the top right when the host cannot say where the icon is", () => {
    expect(flightTarget(undefined, { width: 1470, height: 956 })).toEqual({
      x: 1350,
      y: 12,
    })
    expect(flightTarget({ x: Number.NaN, y: 0 }, { width: 1470, height: 956 })).toEqual({
      x: 1350,
      y: 12,
    })
    expect(flightTarget({ x: 1400, y: 11 }, { width: 1470, height: 956 })).toEqual({
      x: 1400,
      y: 11,
    })
  })

  it("draws nothing for a window with no size", () => {
    expect(
      flightFrames({ left: 0, top: 0, width: 0, height: 0 }, { x: 0, y: 0 }),
    ).toEqual([])
  })
})
