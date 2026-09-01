/**
 * A dev-only surface for choosing the app icon, served at /icon.html by
 * `pnpm dev`. It is not part of the production bundle — only index.html is.
 *
 * The direction is in design.md: a soft fluid wash, grain, and the agent's
 * circular painting on top. No line work — the component's `contours`, `waves`
 * and `rings` were all rendered here against these palettes and rejected on
 * sight, so `pattern="none"` is not an oversight.
 *
 * This composes the real `GradientSurface` rather than a mock, so the shipped
 * icon has something exact to match. It cannot *be* the shipped icon: that is
 * rasterised by resvg, which runs no CSS.
 *
 * Everything is drawn at three sizes, because a wash that reads beautifully at
 * 224px can collapse into a coloured square at 16px — which is the whole reason
 * the tray icon is a different painting today.
 */
import * as React from "react"
import { createRoot } from "react-dom/client"
import { GradientSurface } from "@nessa-ui/react/gradient-surface"
import { RandomAvatar, type RandomAvatarTone } from "@nessa-ui/react/random-avatar"

import {
  AGENT_HUES,
  AGENT_ICON_TONE,
  AGENT_ICON_WASH,
  AGENT_SEED,
} from "./conversation/model"
import "./styles.css"

/**
 * The rim fade. Opaque until well past halfway so the painting keeps its
 * substance, then out to nothing by the edge — the disc stops having an edge
 * rather than having a soft one.
 */
const FEATHER = "radial-gradient(circle, #000 62%, rgba(0,0,0,0.55) 84%, transparent 97%)"
/** The same idea, given up earlier — the paint thins from halfway out. */
const SOFT_FEATHER =
  "radial-gradient(circle, #000 34%, rgba(0,0,0,0.6) 62%, rgba(0,0,0,0.2) 84%, transparent 96%)"

/**
 * The shipped wash first, then the rest of the candidates for comparison. The
 * design system's own presets are all deep; this direction is pale, so these
 * are the app's own.
 */
const palettes = {
  blush: AGENT_ICON_WASH,
  sorbet: ["#b3a2e2", "#cdb4ee", "#f0bcae", "#f7ab5e"],
  haze: ["#8fa6e8", "#bda5ea", "#f2ab8e", "#fbb26c"],
  dawn: ["#c9bce9", "#dcc8f0", "#f6d0bd", "#fbc38a"],
} as const

type Palette = keyof typeof palettes

/**
 * How the painting meets the wash.
 *
 * `RandomAvatar` has no transparent ground — only `paper` and `ink` — so the
 * painting always arrives as washes laid on a near-white circle, and that
 * circle is what read as a disc stuck onto the surface. Multiplying the whole
 * thing removes it without touching the component: white is multiply's
 * identity, so the paper falls away against the wash and only the pigment,
 * which the component itself already lays down with `mix-blend-multiply`,
 * survives to tint the gradient.
 *
 * What is left is clipped to a circle, so the feather still earns its place:
 * it is what stops the paint ending on a hard arc where a stroke runs out to
 * the edge.
 *
 * Blend modes are safe inside the surface: `GradientSurface`'s root is
 * `isolate`, so a blended child composites against the wash and stops there
 * rather than reaching the page behind it.
 */
const blends = {
  /** Paint in the gradient, no circle: the wash carries it. */
  painted: {
    mixBlendMode: "multiply",
    maskImage: FEATHER,
    WebkitMaskImage: FEATHER,
  },
  /** The same, feathered harder — barely a boundary at all. */
  "painted, softer": {
    mixBlendMode: "multiply",
    maskImage: SOFT_FEATHER,
    WebkitMaskImage: SOFT_FEATHER,
  },
  /** Multiply alone, to show what the circular clip is still doing. */
  "painted, unfeathered": { mixBlendMode: "multiply" },
  /** The previous answer, kept for comparison: the paper circle still there. */
  "disc, feathered": {
    maskImage: FEATHER,
    WebkitMaskImage: FEATHER,
  },
} as const satisfies Record<string, React.CSSProperties>

type Blend = keyof typeof blends

/** The rounded square macOS draws app icons in, with the painting on it. */
function Icon({
  palette,
  size,
  grain = 1.2,
  blend = "painted",
  tone = AGENT_ICON_TONE,
  scale = 0.62,
}: {
  palette: Palette
  size: number
  grain?: number
  blend?: Blend
  tone?: RandomAvatarTone
  scale?: number
}) {
  return (
    <GradientSurface
      colors={palettes[palette]}
      pattern="none"
      grain={grain}
      // Only shape and shadow travel through className: the component is a
      // grid, and a display utility here would silently replace it.
      className="rounded-[22.5%] shadow-lg ring-1 ring-black/5"
      style={{ width: size, height: size }}
    >
      <div className="grid h-full place-items-center">
        <RandomAvatar
          seed={AGENT_SEED}
          hues={AGENT_HUES}
          tone={tone}
          ground="paper"
          className="rounded-full"
          style={{
            width: size * scale,
            height: size * scale,
            ...blends[blend],
          }}
        />
      </div>
    </GradientSurface>
  )
}

/** The sizes that decide whether a wash works, in one row. */
function Sizes({ palette }: { palette: Palette }) {
  return (
    <div className="flex items-end gap-3">
      <Icon palette={palette} size={128} />
      <Icon palette={palette} size={64} />
      {/* Menu-bar blue, where a wash has to survive being 16px tall. */}
      <div className="flex items-center gap-2 rounded bg-[#2b6cb0] px-2 py-1">
        <Icon palette={palette} size={16} />
        <Icon palette={palette} size={20} />
      </div>
    </div>
  )
}

function Preview() {
  return (
    // The app's stylesheet locks the window's scroll; this page is a document.
    <div className="h-full overflow-auto bg-background p-6">
      <header className="mb-6">
        <h1 className="nessa-text-4 font-medium text-foreground">
          App icon — soft fluid systems
        </h1>
        <p className="nessa-text-2 mt-1 max-w-prose text-muted-foreground">
          The wash and the grain are the real <code>GradientSurface</code>; the disc is
          the same painting the transcript uses. See design.md.
        </p>
      </header>

      <section className="mb-10">
        <h2 className="nessa-text-3 mb-3 font-medium text-foreground">blush — shipped</h2>
        <div className="flex flex-wrap items-end gap-6">
          <Icon palette="blush" size={224} />
          <Sizes palette="blush" />
        </div>
      </section>

      <section className="mb-10">
        <h2 className="nessa-text-3 mb-1 font-medium text-foreground">
          How the painting meets the wash
        </h2>
        <p className="nessa-text-1 mb-3 max-w-prose text-muted-foreground">
          The component has no transparent ground, so the painting always arrives on a
          near-white circle. Multiplying drops that circle without touching the component
          — white is multiply&rsquo;s identity — and leaves only the pigment in the
          gradient. Shown large, and at 32px where thin paint can vanish altogether.
        </p>
        <div className="flex flex-wrap items-end gap-5">
          {(Object.keys(blends) as Blend[]).map((blend) => (
            <div key={blend} className="flex flex-col items-center gap-1.5">
              <Icon palette="blush" size={148} blend={blend} />
              <Icon palette="blush" size={32} blend={blend} />
              <span className="nessa-text-1 text-muted-foreground">{blend}</span>
            </div>
          ))}
        </div>
      </section>

      <section className="mb-10">
        <h2 className="nessa-text-3 mb-1 font-medium text-foreground">Pigment</h2>
        <p className="nessa-text-1 mb-3 max-w-prose text-muted-foreground">
          Without the white circle behind it the paint has to carry on its own, so how
          dilute it is matters more than it did. The icon ships{" "}
          <code>{AGENT_ICON_TONE}</code> today.
        </p>
        <div className="flex flex-wrap items-end gap-4">
          {(["pastel", "soft", "vivid"] as const).map((tone) => (
            <div key={tone} className="flex flex-col items-center gap-1.5">
              <Icon palette="blush" size={148} tone={tone} />
              <Icon palette="blush" size={32} tone={tone} />
              <span className="nessa-text-1 text-muted-foreground">{tone}</span>
            </div>
          ))}
        </div>
      </section>

      <section className="mb-10">
        <h2 className="nessa-text-3 mb-1 font-medium text-foreground">Grain</h2>
        <p className="nessa-text-1 mb-3 text-muted-foreground">
          None, shipped, and heavier — what keeps a large soft wash from reading as a
          screensaver.
        </p>
        <div className="flex flex-wrap items-end gap-4">
          {[0, 1.2, 2.4].map((grain) => (
            <div key={grain} className="flex flex-col items-center gap-1.5">
              <Icon palette="blush" size={128} grain={grain} />
              <span className="nessa-text-1 text-muted-foreground">grain {grain}</span>
            </div>
          ))}
        </div>
      </section>

      {/* The painting is generated in JS, so the shipped SVG cannot be written
          by hand — it is lifted from here, at exactly the settings the icon
          ships, by `scripts/export-painting.mjs`. Kept visible rather than
          hidden so a change to the identity is seen rather than discovered. */}
      <section className="mb-10">
        <h2 className="nessa-text-3 mb-1 font-medium text-foreground">
          The painting, as exported
        </h2>
        <p className="nessa-text-1 mb-3 max-w-plain text-muted-foreground">
          Seed <code>{AGENT_SEED}</code>, tone <code>{AGENT_ICON_TONE}</code>. This exact
          node becomes the icon&rsquo;s paint layer.
        </p>
        <RandomAvatar
          id="icon-painting"
          seed={AGENT_SEED}
          hues={AGENT_HUES}
          tone={AGENT_ICON_TONE}
          ground="paper"
          className="size-[148px] rounded-full"
        />
      </section>

      <section>
        <h2 className="nessa-text-3 mb-3 font-medium text-foreground">Not chosen</h2>
        <div className="flex flex-wrap gap-6">
          {(Object.keys(palettes) as Palette[])
            .filter((palette) => palette !== "blush")
            .map((palette) => (
              <div key={palette} className="flex flex-col items-center gap-1.5">
                <Icon palette={palette} size={128} />
                <span className="nessa-text-1 text-muted-foreground">{palette}</span>
              </div>
            ))}
        </div>
      </section>
    </div>
  )
}

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

createRoot(container).render(<Preview />)
