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
 * same frame as the resize rather than a render later.
 */
export function NightScene() {
  const frameRef = useRef<HTMLPreElement>(null)

  useLayoutEffect(() => {
    const frame = frameRef.current
    if (!frame) return
    const naturalHeight = nightSceneSillRows * nightSceneLayoutFontPx
    const measure = () => {
      // The content height: the box's padding holds the faded rows, whose size
      // follows the scale, so it must not feed back into it.
      const height = Number.parseFloat(getComputedStyle(frame).height)
      const scale = nightSceneScale(height, naturalHeight)
      frame.style.setProperty("--night-scene-scale", String(scale))
      frame.style.setProperty("--night-scene-em", `${scale * nightSceneLayoutFontPx}px`)
      frame.dataset.measured = ""
    }
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(frame)
    return () => observer.disconnect()
  }, [])

  return (
    <pre
      ref={frameRef}
      aria-hidden="true"
      className="desktop-night-scene"
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
        <Rain depth="far" pattern={nightSceneRainFar} />
        <Rain depth="mid" pattern={nightSceneRainMid} />
        <Rain depth="near" pattern={nightSceneRainNear} />
        <span>{nightSceneDim.join("\n")}</span>
        <span data-lit="">{nightSceneLit.join("\n")}</span>
        <Steam />
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
}: {
  depth: "far" | "mid" | "near"
  pattern: readonly string[]
}) {
  const text = pattern.join("\n")
  return (
    <span className="desktop-night-scene-rain" data-depth={depth}>
      <span>{`${text}\n${text}`}</span>
    </span>
  )
}

/** Steam over the mug: the tiling wisps doubled, rising by one copy in a loop. */
function Steam() {
  const text = nightSceneSteam.pattern.join("\n")
  return (
    <span className="desktop-night-scene-steam">
      <span>{`${text}\n${text}`}</span>
    </span>
  )
}
