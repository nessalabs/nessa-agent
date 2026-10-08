import { useLayoutEffect, useRef, type CSSProperties } from "react"
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
 * same frame as the resize rather than a render later. `still`: its rain and
 * steam hold still, as they do in a conversation pane's sliver of it.
 */
export function NightScene({ still = false }: { still?: boolean }) {
  const frameRef = useRef<HTMLPreElement>(null)

  useLayoutEffect(() => {
    const frame = frameRef.current
    if (!frame) return
    const naturalHeight = nightSceneSillRows * nightSceneLayoutFontPx
    const observer = new ResizeObserver((entries) => {
      const entry = entries.find((entry) => entry.target === frame)
      if (!entry) return
      // The content height: the box's padding holds the faded rows, whose size
      // follows the scale, so it must not feed back into it. The observer owns
      // this measurement; reading computed height here would flush layout again.
      const scale = nightSceneScale(entry.contentRect.height, naturalHeight)
      frame.style.setProperty("--night-scene-scale", String(scale))
      frame.style.setProperty("--night-scene-em", `${scale * nightSceneLayoutFontPx}px`)
      frame.dataset.measured = ""
    })
    observer.observe(frame)
    return () => observer.disconnect()
  }, [])

  return (
    <pre
      ref={frameRef}
      aria-hidden="true"
      className="desktop-night-scene"
      data-still={still || undefined}
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
          fontSize: `${nightSceneLayoutFontPx}px`,
        } as CSSProperties
      }
    >
      <span className="desktop-night-scene-layers">
        <Rain depth="far" pattern={nightSceneRainFar} still={still} />
        <Rain depth="mid" pattern={nightSceneRainMid} still={still} />
        <Rain depth="near" pattern={nightSceneRainNear} still={still} />
        <span>{nightSceneDim.join("\n")}</span>
        <span data-lit="">{nightSceneLit.join("\n")}</span>
        <Steam still={still} />
      </span>
    </pre>
  )
}

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
