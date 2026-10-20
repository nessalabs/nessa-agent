import { memo, useLayoutEffect, useRef, useState, type CSSProperties } from "react"
import {
  nightSceneDim,
  nightSceneFadeRows,
  nightSceneLayoutFontPx,
  nightSceneLit,
  nightSceneMug,
  nightSceneRainFar,
  nightSceneRainHeights,
  nightSceneRainMid,
  nightSceneRainNear,
  nightSceneScale,
  nightSceneSillRows,
  nightSceneSteam,
  nightSceneWindow,
} from "../model/night-scene"

const sceneColumns = Math.max(
  ...nightSceneDim.map((line) => line.length),
  ...nightSceneLit.map((line) => line.length),
)

/** The layer in two mounts, so neither frame lays out every line. */
function halves(lines: readonly string[]): readonly [string, string] {
  const mid = Math.ceil(lines.length / 2)
  return [lines.slice(0, mid).join("\n"), lines.slice(mid).join("\n")]
}

const [dimTop, dimBottom] = halves(nightSceneDim)
const [litTop, litBottom] = halves(nightSceneLit)

type ScenePhase = "shell" | "dim" | "dim2" | "lit" | "lit2" | "full"

const nextPhase: Partial<Record<ScenePhase, ScenePhase>> = {
  dim: "dim2",
  dim2: "lit",
  lit: "lit2",
  lit2: "full",
}

/**
 * The home screen's night scene, set as text: the dim layer with the lit layer
 * over it. Decoration only: hidden from assistive technology and from the
 * pointer.
 *
 * Its box takes its height from the workspace's height alone (`styles.css`),
 * so nothing below it moves while the window is resized sideways or a sidebar
 * slides. The scene is laid out once at `nightSceneLayoutFontPx` and scaled
 * around its sill, which stays on the box's bottom edge, to fill the box's
 * height; sideways it is only cropped. The scale is `nightSceneScale`,
 * written straight to the element from a ResizeObserver so it lands in the
 * same frame as the resize rather than a render later. The lines themselves
 * wait two frames (`data-scene-ready`): the frame that measures the box, and
 * the frame that focuses the new pane, each stay clear of that layout. The
 * ready frame mounts the top half of the dim layer and fixes the layers'
 * width at the text's column count. The bottom half, then each half of the
 * lit layer, then the rain and steam (`data-scene-complete`), each take the
 * next frame. A later change of pane width only crops them. `still`: its
 * rain and steam hold still, as they do in a conversation pane's sliver of it.
 */
export const NightScene = memo(function NightScene({
  still = false,
}: {
  still?: boolean
}) {
  const frameRef = useRef<HTMLPreElement>(null)
  const metrics = useRef({
    scale: "1",
    em: `${nightSceneLayoutFontPx}px`,
    measured: false,
  })
  const [phase, setPhase] = useState<ScenePhase>("shell")

  useLayoutEffect(() => {
    const frame = frameRef.current
    if (!frame) return
    const naturalHeight = nightSceneSillRows * nightSceneLayoutFontPx
    let readyFrame = 0
    let started = false
    const observer = new ResizeObserver((entries) => {
      const entry = entries.find((entry) => entry.target === frame)
      if (!entry) return
      // The content height: the box's padding holds the faded rows, whose size
      // follows the scale, so it must not feed back into it. The observer owns
      // this measurement; reading computed height here would flush layout again.
      const height = entry.contentRect.height
      const scale = nightSceneScale(height, naturalHeight)
      const em = `${scale * nightSceneLayoutFontPx}px`
      metrics.current = { scale: String(scale), em, measured: true }
      frame.style.setProperty("--night-scene-scale", metrics.current.scale)
      frame.style.setProperty("--night-scene-em", em)
      frame.dataset.measured = ""
      // A zero box has nothing to lay out. The lines wait two frames after
      // the first positive height. The text is mounted then, so this render
      // carries the scale the observer wrote and does not wipe it.
      if (height > 0 && !started && readyFrame === 0) {
        started = true
        readyFrame = requestAnimationFrame(() => {
          readyFrame = requestAnimationFrame(() => {
            readyFrame = 0
            if (!frame.isConnected) return
            setPhase("dim")
          })
        })
      }
    })
    observer.observe(frame)
    return () => {
      observer.disconnect()
      if (readyFrame !== 0) cancelAnimationFrame(readyFrame)
    }
  }, [])

  useLayoutEffect(() => {
    const next = nextPhase[phase]
    if (!next) return
    const id = requestAnimationFrame(() => setPhase(next))
    return () => cancelAnimationFrame(id)
  }, [phase])

  const shown = metrics.current
  return (
    <pre
      ref={frameRef}
      aria-hidden="true"
      className="desktop-night-scene"
      data-still={still || undefined}
      data-measured={shown.measured ? "" : undefined}
      data-scene-ready={phase === "shell" ? undefined : ""}
      data-scene-complete={phase === "full" ? "" : undefined}
      style={
        {
          "--night-scene-sill-rows": nightSceneSillRows,
          "--night-scene-fade-rows": nightSceneFadeRows,
          "--night-scene-window-column": nightSceneWindow.column,
          "--night-scene-window-columns": nightSceneWindow.columns,
          "--night-scene-window-rows": nightSceneWindow.rows,
          "--night-scene-rain-rows": nightSceneWindow.rows * nightSceneRainHeights,
          "--night-scene-mug-column": nightSceneMug.column,
          "--night-scene-mug-columns": nightSceneMug.columns,
          "--night-scene-mug-row": nightSceneMug.row,
          "--night-scene-steam-column": nightSceneSteam.column,
          "--night-scene-steam-row": nightSceneSteam.row,
          "--night-scene-steam-rows": nightSceneSteam.pattern.length,
          "--night-scene-scale": shown.scale,
          "--night-scene-em": shown.em,
          fontSize: `${nightSceneLayoutFontPx}px`,
        } as CSSProperties
      }
    >
      <span className="desktop-night-scene-layers" style={{ width: `${sceneColumns}ch` }}>
        {phase === "full" ? (
          <Rain key="far" depth="far" pattern={nightSceneRainFar} still={still} />
        ) : null}
        {phase === "full" ? (
          <Rain key="mid" depth="mid" pattern={nightSceneRainMid} still={still} />
        ) : null}
        {phase === "full" ? (
          <Rain key="near" depth="near" pattern={nightSceneRainNear} still={still} />
        ) : null}
        {phase === "shell" ? null : (
          <span key="dim" className="desktop-night-scene-copy" data-dim="">
            <span>{dimTop}</span>
            {phase === "dim" ? null : <span>{dimBottom}</span>}
          </span>
        )}
        {phase === "lit" || phase === "lit2" || phase === "full" ? (
          <span key="lit" className="desktop-night-scene-copy" data-lit="">
            <span>{litTop}</span>
            {phase === "lit" ? null : <span>{litBottom}</span>}
          </span>
        ) : null}
        {phase === "full" ? <Steam key="steam" still={still} /> : null}
      </span>
    </pre>
  )
})

/**
 * One depth of rain in the window opening: two copies of a vertically tiling
 * pattern, one above the other, sliding down by one copy's height in a loop.
 */
function Rain({
  depth,
  pattern,
  still,
}: {
  depth: "far" | "mid" | "near"
  pattern: readonly string[]
  still: boolean
}) {
  // A static window sees only its first rows. The copies below the clip exist
  // for an animation's travel; laying them out in every header wastes work.
  const text = (still ? pattern.slice(0, nightSceneWindow.rows) : pattern).join("\n")
  return (
    <span className="desktop-night-scene-rain" data-depth={depth}>
      <span>{still ? text : `${text}\n${text}`}</span>
    </span>
  )
}

/** Steam over the mug: the tiling wisps doubled, rising by one copy in a loop. */
function Steam({ still }: { still: boolean }) {
  const text = nightSceneSteam.pattern.join("\n")
  return (
    <span className="desktop-night-scene-steam">
      <span>{still ? text : `${text}\n${text}`}</span>
    </span>
  )
}
