/**
 * A picture's palette, and the window theme it implies.
 *
 * Self-contained on purpose: it takes raw RGBA pixels and returns colours, and
 * touches no DOM, canvas, or storage, so it can move into a package of its own.
 * Colours are worked in OKLab/OKLCH, where distance and lightness match what
 * the eye sees, so clustering finds the colours a person would name and the
 * theme's lightness can be set without shifting hue.
 */

/** A colour in OKLCH: lightness 0–1, chroma (vividness) 0–~0.4, hue in degrees. */
export interface Oklch {
  l: number
  c: number
  h: number
}

/** One of a picture's main colours, and the share of the picture it covers. */
export interface PaletteColor {
  color: Oklch
  weight: number
}

/** The three colours a window theme is made of (see `styles.css`). */
export interface ImageTheme {
  /** The light from above, a CSS colour with its own transparency. */
  high: string
  /** The light from below. */
  low: string
  /** Glows, focus halos, and selection. */
  edge: string
}

type Lab = [number, number, number]

function linear(channel: number) {
  const v = channel / 255
  return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4
}

/** sRGB bytes to OKLab (Björn Ottosson's matrices). */
export function rgbToOklab(r: number, g: number, b: number): Lab {
  const lr = linear(r)
  const lg = linear(g)
  const lb = linear(b)
  const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb)
  const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb)
  const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb)
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ]
}

function toOklch([l, a, b]: Lab): Oklch {
  const hue = (Math.atan2(b, a) * 180) / Math.PI
  return { l, c: Math.hypot(a, b), h: hue < 0 ? hue + 360 : hue }
}

function distance(x: Lab, y: Lab) {
  return (x[0] - y[0]) ** 2 + (x[1] - y[1]) ** 2 + (x[2] - y[2]) ** 2
}

/** The smaller angle between two hues, 0–180. */
export function hueDistance(a: number, b: number) {
  const d = Math.abs(a - b) % 360
  return d > 180 ? 360 - d : d
}

/**
 * The picture's main colours, largest share first. `pixels` is RGBA bytes, as
 * a canvas returns them; transparent pixels are skipped. k-means in OKLab,
 * started from colours spread across the picture (each next start the sample
 * farthest from those chosen), so the same picture always gives the same
 * palette.
 */
export function extractPalette(
  pixels: ArrayLike<number>,
  count = 5,
  iterations = 12,
): PaletteColor[] {
  const samples: Lab[] = []
  for (let i = 0; i + 3 < pixels.length; i += 4) {
    if (pixels[i + 3] < 128) continue
    samples.push(rgbToOklab(pixels[i], pixels[i + 1], pixels[i + 2]))
  }
  if (samples.length === 0) return []

  const centres: Lab[] = [samples[Math.floor(samples.length / 2)]]
  const nearest = samples.map((sample) => distance(sample, centres[0]))
  while (centres.length < Math.min(count, samples.length)) {
    let farthest = 0
    for (let i = 1; i < samples.length; i += 1) {
      if (nearest[i] > nearest[farthest]) farthest = i
    }
    if (nearest[farthest] === 0) break
    centres.push(samples[farthest])
    samples.forEach((sample, i) => {
      nearest[i] = Math.min(nearest[i], distance(sample, samples[farthest]))
    })
  }

  const owner = new Array<number>(samples.length).fill(0)
  for (let round = 0; round < iterations; round += 1) {
    samples.forEach((sample, i) => {
      let best = 0
      for (let k = 1; k < centres.length; k += 1) {
        if (distance(sample, centres[k]) < distance(sample, centres[best])) best = k
      }
      owner[i] = best
    })
    const sums = centres.map((): [number, number, number, number] => [0, 0, 0, 0])
    samples.forEach((sample, i) => {
      const sum = sums[owner[i]]
      sum[0] += sample[0]
      sum[1] += sample[1]
      sum[2] += sample[2]
      sum[3] += 1
    })
    sums.forEach((sum, k) => {
      if (sum[3] > 0) centres[k] = [sum[0] / sum[3], sum[1] / sum[3], sum[2] / sum[3]]
    })
  }

  const shares = centres.map(() => 0)
  owner.forEach((k) => {
    shares[k] += 1
  })
  return centres
    .map((centre, k) => ({ color: toOklch(centre), weight: shares[k] / samples.length }))
    .filter((entry) => entry.weight > 0)
    .sort((a, b) => b.weight - a.weight)
}

/** Below this chroma a colour reads as grey; a picture of only these is neutral. */
const greyChroma = 0.03

/** Hues closer than this read as one hue, so the low light looks elsewhere. */
const sameHue = 30

/** How far the low light turns from the edge when the picture offers no second hue. */
const analogousTurn = 40

const clamp = (value: number, low: number, high: number) =>
  Math.min(high, Math.max(low, value))

const css = ({ l, c, h }: Oklch, alpha?: number) =>
  `oklch(${l.toFixed(3)} ${c.toFixed(3)} ${h.toFixed(1)}${alpha === undefined ? "" : ` / ${alpha}%`})`

/**
 * The window theme a palette implies, within the ranges the built-in themes
 * keep so no picture can make the window garish or unreadable:
 *
 * - the edge takes the colour that is both vivid and plentiful, the one a
 *   person would say the picture "is", made light;
 * - the light from above is the same hue, dimmer and translucent;
 * - the light from below is the most prominent colour of a different hue, or,
 *   when the picture has only one, a neighbouring (analogous) hue;
 * - a picture with no real colour gives a neutral, graphite-like theme.
 */
export function themeFromPalette(palette: readonly PaletteColor[]): ImageTheme {
  const coloured = palette.filter((entry) => entry.color.c >= greyChroma)
  if (coloured.length === 0) {
    const hue = palette[0]?.color.h ?? 250
    return {
      high: css({ l: 0.78, c: 0.006, h: hue }, 16),
      low: css({ l: 0.65, c: 0.01, h: hue }, 9),
      edge: css({ l: 0.92, c: 0.004, h: hue }),
    }
  }

  const score = (entry: PaletteColor) => entry.color.c * Math.sqrt(entry.weight)
  const lead = coloured.reduce((best, entry) =>
    score(entry) > score(best) ? entry : best,
  )
  const second = coloured
    .filter((entry) => hueDistance(entry.color.h, lead.color.h) >= sameHue)
    .sort((a, b) => b.weight - a.weight)[0]

  const hue = lead.color.h
  const chroma = clamp(lead.color.c, 0.04, 0.14)
  const lowHue = second ? second.color.h : (hue + analogousTurn) % 360
  const lowChroma = clamp(second ? second.color.c : lead.color.c * 0.8, 0.03, 0.13)
  return {
    high: css({ l: 0.6, c: chroma, h: hue }, 32),
    low: css({ l: 0.6, c: lowChroma, h: lowHue }, 18),
    edge: css({ l: 0.85, c: clamp(chroma * 0.75, 0.03, 0.1), h: hue }),
  }
}

/** A palette colour as a CSS colour, for showing it. */
export function paletteCss(color: Oklch) {
  return css(color)
}
