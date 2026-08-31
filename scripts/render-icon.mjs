/**
 * Rasterises an SVG to a PNG with a genuinely transparent background.
 *
 * This exists because `qlmanage -t`, the obvious macOS one-liner, is a
 * QuickLook *thumbnailer*: it composites onto white. The PNG it produces has an
 * alpha channel — `sips -g hasAlpha` says yes — but every pixel is opaque, so
 * the icon carried a white square into the menu bar. resvg renders the SVG
 * itself, including the feTurbulence/feDisplacementMap filters the avatar is
 * painted with, and leaves everything outside the artwork transparent.
 *
 *   node scripts/render-icon.mjs <input.svg> <output.png> <size>
 */
import { readFileSync, writeFileSync } from "node:fs"
import { Resvg } from "@resvg/resvg-js"

/**
 * resvg does not implement CSS Color 4, so an `oklch()` fill resolves to black
 * and the whole avatar renders as a solid disc. The design system paints in
 * oklch, so the colours are converted to sRGB before it ever sees them.
 *
 * oklch → oklab → LMS → linear sRGB → gamma-encoded sRGB, per the Oklab
 * definition. Hues outside 0–360 are fine: cos and sin are periodic.
 */
function oklchToRgb(lightness, chroma, hueDegrees) {
  const h = (hueDegrees * Math.PI) / 180
  const a = chroma * Math.cos(h)
  const b = chroma * Math.sin(h)

  const l = (lightness + 0.3963377774 * a + 0.2158037573 * b) ** 3
  const m = (lightness - 0.1055613458 * a - 0.0638541728 * b) ** 3
  const s = (lightness - 0.0894841775 * a - 1.291485548 * b) ** 3

  const linear = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ]

  return linear.map((channel) => {
    const encoded =
      channel <= 0.0031308
        ? 12.92 * channel
        : 1.055 * Math.abs(channel) ** (1 / 2.4) - 0.055
    return Math.round(Math.min(1, Math.max(0, encoded)) * 255)
  })
}

const OKLCH = /oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*\)/g

function toSrgb(svg) {
  return svg.replace(OKLCH, (_, l, c, h) => {
    const [r, g, b] = oklchToRgb(Number(l), Number(c), Number(h))
    return `rgb(${r},${g},${b})`
  })
}

const [input, output, size] = process.argv.slice(2)
if (!input || !output || !size) {
  console.error("usage: node scripts/render-icon.mjs <input.svg> <output.png> <size>")
  process.exit(1)
}

const png = new Resvg(toSrgb(readFileSync(input, "utf8")), {
  fitTo: { mode: "width", value: Number(size) },
  // Nothing is painted behind the artwork; the transparent default is the
  // whole point of not using qlmanage.
  font: { loadSystemFonts: false },
}).render().asPng()

writeFileSync(output, png)
console.log(`${output}  ${size}x${size}  ${(png.length / 1024).toFixed(1)}KB`)
