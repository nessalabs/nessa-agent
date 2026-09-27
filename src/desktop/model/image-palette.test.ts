import { describe, expect, it } from "vitest"
import {
  extractPalette,
  hueDistance,
  rgbToOklab,
  themeFromPalette,
  type PaletteColor,
} from "./image-palette"

/** RGBA bytes for `count` pixels of each colour. */
function pixels(...runs: [r: number, g: number, b: number, count: number][]) {
  const out: number[] = []
  for (const [r, g, b, count] of runs) {
    for (let i = 0; i < count; i += 1) out.push(r, g, b, 255)
  }
  return out
}

const chroma = (css: string) => Number(css.split(" ")[1])
const lightness = (css: string) => Number(css.slice("oklch(".length).split(" ")[0])
const hue = (css: string) => Number(css.split(" ")[2].replace(")", ""))

describe("rgbToOklab", () => {
  it("matches OKLab's reference white and black", () => {
    const [l, a, b] = rgbToOklab(255, 255, 255)
    expect(l).toBeCloseTo(1, 3)
    expect(Math.abs(a)).toBeLessThan(1e-3)
    expect(Math.abs(b)).toBeLessThan(1e-3)
    expect(rgbToOklab(0, 0, 0)[0]).toBeCloseTo(0, 5)
  })
})

describe("extractPalette", () => {
  it("finds a picture's colours and their shares, largest first", () => {
    const palette = extractPalette(pixels([200, 40, 40, 300], [30, 60, 200, 100]), 2)
    expect(palette).toHaveLength(2)
    expect(palette[0].weight).toBeCloseTo(0.75)
    expect(palette[1].weight).toBeCloseTo(0.25)
    expect(hueDistance(palette[0].color.h, 29)).toBeLessThan(15) // red
    expect(hueDistance(palette[1].color.h, 264)).toBeLessThan(15) // blue
  })

  it("gives the same palette for the same picture", () => {
    const picture = pixels([250, 180, 60, 50], [20, 120, 90, 80], [240, 240, 240, 30])
    expect(extractPalette(picture)).toEqual(extractPalette(picture))
  })

  it("skips transparent pixels, and has nothing for a picture with none left", () => {
    const clear = [255, 0, 0, 0, 255, 0, 0, 10]
    expect(extractPalette(clear)).toEqual([])
  })

  it("returns no more colours than the picture has", () => {
    expect(extractPalette(pixels([10, 200, 10, 40]), 5)).toHaveLength(1)
  })
})

describe("themeFromPalette", () => {
  const entry = (l: number, c: number, h: number, weight: number): PaletteColor => ({
    color: { l, c, h },
    weight,
  })

  it("leads with the colour that is both vivid and plentiful", () => {
    const theme = themeFromPalette([
      entry(0.2, 0.01, 250, 0.7), // a large grey
      entry(0.7, 0.15, 60, 0.3), // warm light
    ])
    expect(hue(theme.edge)).toBeCloseTo(60)
    expect(hue(theme.high)).toBeCloseTo(60)
  })

  it("takes the low light from a second hue when the picture has one", () => {
    const theme = themeFromPalette([entry(0.5, 0.12, 250, 0.6), entry(0.6, 0.1, 30, 0.4)])
    expect(hue(theme.low)).toBeCloseTo(30)
  })

  it("turns to a neighbouring hue when the picture has only one", () => {
    const theme = themeFromPalette([
      entry(0.5, 0.12, 250, 0.6),
      entry(0.6, 0.1, 260, 0.4),
    ])
    expect(hueDistance(hue(theme.low), 290)).toBeLessThan(1)
  })

  it("keeps a garish picture within the built-in themes' ranges", () => {
    const theme = themeFromPalette([entry(0.6, 0.35, 140, 1)])
    expect(chroma(theme.high)).toBeLessThanOrEqual(0.14)
    expect(chroma(theme.edge)).toBeLessThanOrEqual(0.1)
    expect(lightness(theme.edge)).toBeCloseTo(0.85)
  })

  it("gives a neutral theme for a picture with no real colour", () => {
    const theme = themeFromPalette([
      entry(0.3, 0.005, 90, 0.8),
      entry(0.9, 0.01, 90, 0.2),
    ])
    expect(chroma(theme.edge)).toBeLessThan(0.01)
    expect(chroma(theme.high)).toBeLessThan(0.01)
  })

  it("gives a neutral theme for an empty palette", () => {
    expect(chroma(themeFromPalette([]).edge)).toBeLessThan(0.01)
  })
})
