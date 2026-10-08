/**
 * The frame sampler `drag.mjs` runs inside the page. One function, so the
 * page and the probe (`shape-sampler.test.mjs`) share the loop it owns.
 *
 * Each call takes a generation. `stopShapeFrames` retires it. A tick whose
 * generation is no longer current records nothing and does not schedule
 * another — a recorder stopped and started again inside one frame cannot
 * both write the next frame (#368).
 */

/** Starts a recorder. The frames it takes land on `window.__shapes`. */
export function recordShapeFrames(sel) {
  const generation = (window.__shapesGeneration = (window.__shapesGeneration ?? 0) + 1)
  window.__shapes = []
  const rect = (e) => {
    const r = e.getBoundingClientRect()
    return { x: r.left, y: r.top, w: r.width, h: r.height }
  }
  // Every transform from the title up: the scale it is drawn at, across
  // and down.
  const chain = (title) => {
    let sx = 1
    let sy = 1
    for (let e = title; e; e = e.parentElement) {
      const t = getComputedStyle(e).transform
      if (!t || t === "none") continue
      const m = new DOMMatrix(t)
      sx *= Math.hypot(m.a, m.b)
      sy *= Math.hypot(m.c, m.d)
    }
    return { sx, sy }
  }
  // The scale a title is drawn at in this frame, as near as a read can
  // tell. WebKit can update running animations in the middle of a read of
  // computed styles, and the read that straddles the update takes the copy
  // and its content at two moments, seeing a stretch that is not drawn
  // (#365 recorded one frame read 1.0006×1.0009, then 1.012×0.987 over
  // 1.0ms, then 1.0003×1.0004). So the chain is read until two reads in a
  // row agree, and a read whose document clock moved during it is not an
  // agreement: it straddled a sample. The agreed read is judged; it may be
  // a moment later than the frame drawn, which matters only to a stretch
  // that comes and goes within a frame. What this does not hide, as
  // observed in #365 and re-run there whenever this changes: a counter-scale
  // started a frame off still fails `compact-drag-card` in both engines.
  // A chain that never settles in `steadyReads` reads is marked `unsteady`,
  // and every check that judges a title fails it (`unsteadily`): never
  // passed for being unreadable.
  //
  // `synced` is whether every running animation from the title up shares
  // one start time. A small disagreement while they do is one frame of the
  // box and its counter-scale sampling apart (#254). None running, or
  // starts that differ, leaves the strict check in force.
  const steadyReads = 6
  const agree = (a, b) => Math.abs(a.sx - b.sx) <= 0.002 && Math.abs(a.sy - b.sy) <= 0.002
  const clock = () => {
    const time = document.timeline?.currentTime
    return typeof time === "number" ? time : null
  }
  const syncedStarts = (title) => {
    const starts = []
    for (let node = title; node; node = node.parentElement) {
      const animations =
        typeof node.getAnimations === "function" ? node.getAnimations() : []
      for (const animation of animations) {
        if (animation.playState === "finished" || animation.playState === "idle") continue
        if (typeof animation.startTime !== "number") return false
        starts.push(animation.startTime)
      }
    }
    return starts.length > 0 && starts.every((start) => start === starts[0])
  }
  const drawnAt = (title) => {
    const reads = []
    const sample = () => {
      const before = clock()
      const began = performance.now()
      const read = chain(title)
      const after = clock()
      const readMs = performance.now() - began
      reads.push([read.sx, read.sy, readMs])
      return { read, readMs, straddled: before !== null && before !== after }
    }
    let current = sample()
    while (reads.length < steadyReads) {
      const next = sample()
      const steady =
        !current.straddled && !next.straddled && agree(next.read, current.read)
      current = next
      if (steady)
        return {
          sx: current.read.sx,
          sy: current.read.sy,
          readMs: current.readMs,
          reads: reads.length,
          synced: syncedStarts(title),
        }
    }
    return {
      sx: current.read.sx,
      sy: current.read.sy,
      readMs: current.readMs,
      reads: reads.length,
      unsteady: reads,
      synced: syncedStarts(title),
    }
  }
  if (!window.__verifyPointerWatched) {
    window.__verifyPointerWatched = true
    addEventListener(
      "pointermove",
      (e) => (window.__verifyPointer = { x: e.clientX, y: e.clientY, t: e.timeStamp }),
      { capture: true },
    )
  }
  const tick = (t) => {
    if (generation !== window.__shapesGeneration) return
    const ghost = document.querySelector(sel.dragGhost)
    const placeholder = document.querySelector(sel.dragPlaceholder)
    const shown = ghost && ghost.checkVisibility({ checkOpacity: true })
    const copyTitle = ghost?.querySelector(sel.dragTitle)
    window.__shapes.push({
      t,
      pointer: window.__verifyPointer ?? null,
      zone: document.querySelector(sel.dropAnnouncer)?.textContent ?? "",
      ghost: shown ? rect(ghost) : null,
      placeholder: placeholder ? rect(placeholder) : null,
      panes: [...document.querySelectorAll(sel.pane)]
        .filter((e) => !e.closest(sel.dragGhost))
        .map((e) => {
          // Its transcript and composer, drawn. A transcript not painted
          // during the preview is not a part the preview positions (`panes.css`).
          const transcript = e.querySelector(sel.transcript)
          const dock = e.querySelector(sel.dock)
          const inner = transcript?.querySelector(".workspace-transcript-inner")
          const quiet =
            inner instanceof Element &&
            getComputedStyle(inner).contentVisibility === "hidden"
          return {
            key: e.dataset.paneKey,
            ...rect(e),
            transcript: transcript && !quiet ? rect(transcript) : null,
            dock: dock ? rect(dock) : null,
          }
        }),
      titles: [
        ...(shown && copyTitle ? [{ of: "copy", ...drawnAt(copyTitle) }] : []),
        ...[...document.querySelectorAll(sel.pane)]
          .filter((e) => !e.closest(sel.dragGhost) && !e.matches(sel.lifted))
          .map((e) => {
            const title = e.querySelector(sel.titleText)
            return title ? { of: `pane ${e.dataset.paneKey}`, ...drawnAt(title) } : null
          })
          .filter(Boolean),
      ],
    })
    requestAnimationFrame(tick)
  }
  requestAnimationFrame(tick)
}

/** Retires the recorder that is running and returns the frames it took. */
export function stopShapeFrames() {
  window.__shapesGeneration = (window.__shapesGeneration ?? 0) + 1
  return window.__shapes
}

/** The frame taken last, if one has been. */
export function latestShapeFrame() {
  return window.__shapes.at(-1)
}
