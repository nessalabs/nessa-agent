/**
 * A dev-only surface for choosing the app icon, served at /icon.html by
 * `pnpm dev`. It is not part of the production bundle — only index.html is.
 * Each tile renders the same component the app uses for the agent's face at
 * icon size and again at menu-bar size over a menu-bar blue, because a
 * palette that reads well at 96px can collapse into a blob at 16px.
 */
import { createRoot } from "react-dom/client"
import { RandomAvatar, type RandomAvatarTone } from "@nessa-ui/react/random-avatar"

import { AGENT_HUES, AGENT_ICON_TONE, AGENT_SEED } from "./conversation/model"
import "./styles.css"

/** Pastel-leaning hue wheels, brighter than the eight defaults. */
const wheels = {
  /** What the icon and the in-app avatar actually ship with. */
  sorbet: AGENT_HUES,
  candy: [330, 285, 205, 45],
  mint: [160, 195, 250, 300],
} as const

const tones = ["pastel", "soft", "vivid"] as const satisfies readonly RandomAvatarTone[]
type Tone = (typeof tones)[number]
type Wheel = keyof typeof wheels

function Tile({ tone, wheel }: { tone: Tone; wheel: Wheel }) {
  // `paper` rather than `ink`: a light ground has no dark field for a pale
  // wash to punch a white hole in, which is what the first icon did at 16px.
  const props = { seed: AGENT_SEED, tone, ground: "paper", hues: wheels[wheel] } as const
  return (
    <div className="flex flex-col items-center gap-1.5">
      <RandomAvatar {...props} className="size-24 rounded-full" />
      <div className="flex items-center gap-2 rounded bg-[#2b6cb0] px-2 py-1">
        <RandomAvatar {...props} className="size-4 rounded-full" />
        <RandomAvatar {...props} className="size-5 rounded-full" />
      </div>
      <span className="nessa-text-1 text-muted-foreground">
        {wheel} · {tone}
        {wheel === "sorbet" && tone === AGENT_ICON_TONE ? " · shipped" : ""}
      </span>
    </div>
  )
}

function Preview() {
  return (
    // The app's stylesheet locks the window's scroll; this page is a document.
    <div className="h-full overflow-auto bg-background p-6">
      {(Object.keys(wheels) as Wheel[]).map((wheel) => (
        <section key={wheel} className="mb-6">
          <h2 className="nessa-text-3 mb-2 font-medium text-foreground">{wheel}</h2>
          <div className="flex flex-wrap gap-5">
            {tones.map((tone) => (
              <Tile key={tone} tone={tone} wheel={wheel} />
            ))}
          </div>
        </section>
      ))}
    </div>
  )
}

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")
createRoot(container).render(<Preview />)
