/**
 * Composes the app icon as a single SVG: the gradient wash, the agent's
 * painting multiplied into it, and film grain over the whole frame.
 *
 * This exists because the icon has to be built twice. On screen the wash is
 * `GradientSurface`, which paints CSS radial gradients; the shipped icon is
 * rasterised by resvg, which runs no CSS at all. So the same picture is
 * described here in SVG, from the same numbers — `AGENT_ICON_WASH` and the
 * component's own bloom stations — rather than eyeballed into agreement.
 *
 * The painting itself cannot be written by hand: it is generated from a seeded
 * stream in the browser. It is lifted from /icon.html (the "as exported"
 * section) into `nessa-painting.svg` and embedded here verbatim.
 *
 *   node scripts/build-icon.mjs > src-tauri/icons/nessa-icon.svg
 */
import { readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))
const painting = readFileSync(
  resolve(here, "../src-tauri/icons/nessa-painting.svg"),
  "utf8",
)

/**
 * Must match `AGENT_ICON_WASH` in src/agent-identity.ts. Deepest first: the
 * ground floods, the rest are blooms, and the last is centred.
 */
const WASH = ["#d3a9d8", "#e6b6d2", "#f7bfa6", "#f9a86a"]

/**
 * Where each bloom is dropped, copied from `GradientSurface`'s own
 * `bloomStations` so the two pictures agree. `at` and `size` are fractions of
 * the icon's box; `size` is the ellipse's radii, as in CSS.
 */
const STATIONS = [
  { at: [0.5, 0.42], size: [1.18, 0.92] },
  { at: [0.86, 0.08], size: [0.78, 0.82] },
  { at: [0.12, 0.96], size: [0.84, 0.88] },
]

/** Where the wash stops, as CSS `transparent 74%` does. */
const BLOOM_END = 0.74

/**
 * macOS's icon grid: a 1024 canvas with the artwork inset to 824 and a corner
 * radius of 185.4. The inset is not padding to taste — it is what makes the
 * icon sit at the same visual size as every other icon in the Dock.
 */
const CANVAS = 1024
const SHAPE = 824
const RADIUS = 185.4
const INSET = (CANVAS - SHAPE) / 2

/** How much of the icon's face the painting covers, matching the preview. */
const PAINT_SCALE = 0.62

/** Film grain: `GradientSurface` lays it over everything at 0.28 × strength. */
const GRAIN_STRENGTH = 1.2
const GRAIN_OPACITY = 0.28 * GRAIN_STRENGTH
/**
 * Tuned for this canvas rather than copied. The component's tile is 240 CSS px
 * at `baseFrequency` 0.8; the same *visual* grain on a 1024-unit canvas that is
 * displayed a few hundred pixels wide needs a coarser frequency, or the noise
 * falls below a pixel and disappears into an even haze.
 */
const GRAIN_FREQUENCY = 0.55

const round = (value) => Math.round(value * 1000) / 1000

/**
 * One bloom, as a radial gradient. CSS sizes an ellipse by its two radii; SVG
 * gradients are circular, so the ellipse is a scaled circle — the transform
 * squashes around the bloom's own centre so it stays put.
 */
function bloom(id, color, station) {
  const cx = INSET + station.at[0] * SHAPE
  const cy = INSET + station.at[1] * SHAPE
  const rx = station.size[0] * SHAPE
  const ry = station.size[1] * SHAPE
  return `    <radialGradient id="${id}" gradientUnits="userSpaceOnUse" cx="${round(cx)}" cy="${round(cy)}" r="${round(rx)}" gradientTransform="translate(0 ${round(cy)}) scale(1 ${round(ry / rx)}) translate(0 ${round(-cy)})">
      <stop offset="0" stop-color="${color}" stop-opacity="1"/>
      <!-- The colour's own alpha goes to zero rather than fading to a
           transparent black, which would dirty the wash on the way out. -->
      <stop offset="${BLOOM_END}" stop-color="${color}" stop-opacity="0"/>
    </radialGradient>`
}

const blooms = WASH.slice(1).reverse()
const paintSize = SHAPE * PAINT_SCALE
const paintOrigin = (CANVAS - paintSize) / 2

// The painting arrives as a whole `<svg>`; nested, it keeps its own viewBox and
// scales to the box it is given.
const paintLayer = painting
  .trim()
  .replace(
    /^<svg /,
    `<svg x="${round(paintOrigin)}" y="${round(paintOrigin)}" width="${round(paintSize)}" height="${round(paintSize)}" `,
  )

process.stdout
  .write(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${CANVAS} ${CANVAS}" width="${CANVAS}" height="${CANVAS}">
  <defs>
${blooms.map((color, index) => bloom(`bloom-${index}`, color, STATIONS[index])).join("\n")}
    <!-- The rim fade. The painting is clipped to a circle, and without this it
         ends on a hard arc wherever a stroke runs out to the edge. -->
    <radialGradient id="feather">
      <stop offset="0.62" stop-color="#fff"/>
      <stop offset="0.84" stop-color="#8c8c8c"/>
      <stop offset="0.97" stop-color="#000"/>
    </radialGradient>
    <mask id="feather-mask">
      <rect x="${round(paintOrigin)}" y="${round(paintOrigin)}" width="${round(paintSize)}" height="${round(paintSize)}" fill="url(#feather)"/>
    </mask>
    <filter id="grain" x="0" y="0" width="100%" height="100%" color-interpolation-filters="sRGB">
      <feTurbulence type="fractalNoise" baseFrequency="${GRAIN_FREQUENCY}" numOctaves="2" stitchTiles="stitch"/>
      <feColorMatrix type="saturate" values="0"/>
    </filter>
    <clipPath id="face">
      <rect x="${INSET}" y="${INSET}" width="${SHAPE}" height="${SHAPE}" rx="${RADIUS}" ry="${RADIUS}"/>
    </clipPath>
  </defs>

  <!-- Everything is clipped to the rounded square, so the blooms may run past
       the artwork's edge exactly as the CSS ones run past the box. -->
  <g clip-path="url(#face)">
    <rect x="${INSET}" y="${INSET}" width="${SHAPE}" height="${SHAPE}" fill="${WASH[0]}"/>
${blooms
  .map(
    (_color, index) =>
      `    <rect x="${INSET}" y="${INSET}" width="${SHAPE}" height="${SHAPE}" fill="url(#bloom-${index})"/>`,
  )
  .reverse()
  .join("\n")}

    <!-- Multiply is what drops the painting's near-white ground: white is its
         identity, so only the pigment survives to tint the wash. -->
    <g style="mix-blend-mode:multiply" mask="url(#feather-mask)">
      ${paintLayer}
    </g>

    <rect x="${INSET}" y="${INSET}" width="${SHAPE}" height="${SHAPE}" filter="url(#grain)" opacity="${round(GRAIN_OPACITY)}" style="mix-blend-mode:overlay"/>
  </g>
</svg>
`)
