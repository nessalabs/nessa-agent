#!/usr/bin/env node
/**
 * Pane drag and drop (ADR 238, "Drag and drop" — its table is the contract),
 * sampled frame by frame in Chrome and WebKit, in both layouts, at 1440 × 900
 * and 1000 × 700:
 *
 *   sweep-across-zones   the copy's centre stays on the pointer once lifted
 *                        (follows-pointer); no pane leaves the grid or the
 *                        window (inside-grid); each pane moves one way between
 *                        zone changes (one-way); nothing is selected
 *   sideways-top         a sideways sweep near a tall pane's top reaches its side, never above/below
 *   boundary-jitter      ±6px on a zone boundary does not flicker the zone
 *   rest-settles         approached fast, then still for 300 ms: a 1px nudge changes no zone
 *   swap-in-tall-pane    the middle of a tall pane offers Swap
 *   escape-cancels       Escape mid-drag flies the copy home; the release after changes nothing
 *   outside-cancels      a release outside the window changes nothing and leaves nothing lifted
 *   lost-capture-then-move  the page losing the pointer ends the drag; a move and a release
 *                        over a zone after it start nothing and drop nothing
 *   resize-mid-drag      a resize while carrying ends the drag at once: nothing left lifted,
 *                        every pane inside the window every frame, nothing under the controls
 *   command-mid-drag     ⌘W and ⌘0 while carrying end the drag first, then run: the same
 *   overview-session-drop  (sessions in the sidebar) a session carried while the overview
 *                        covers the panes offers no zone and no placeholder; the release changes nothing
 *   flick                down, across and up in one task — before any copy, and again
 *                        lifted but before any preview — changes nothing
 *   chord-right-button   the right button pressed while carrying ends the drag: nothing
 *                        left carried, the release after drops nothing
 *   peek-session-no-zone (sidebar layout) a session carried inside the sidebar revealed
 *                        from the edge offers no zone and no placeholder, the reveal stays
 *                        for the whole drag, and the release changes nothing
 *   copy-under-controls  the corner pane carried: nothing of its copy is painted under the
 *                        window's controls, any frame, and its header is laid out as the
 *                        pane's (the title starts within 2px of where the pane's does)
 *   docked-columns-no-zone  (1440 × 900) a pane carried over the docked sidebar, and over
 *                        the docked session list, offers no zone and no placeholder; the
 *                        release there changes nothing
 *   peek-press-while-hiding  the sidebar revealed from the edge, the pointer leaves it for a
 *                        pane's header and presses within the reveal's 350 ms hide: the
 *                        reveal stays, every frame of the drag; over it is no zone, just past
 *                        it a pane is a target; released away, it hides
 *   reduced-motion       with Motion set to Reduced, a pane carried over the middle of the
 *                        one beside it: that one is drawn in the carried pane's place, at
 *                        once — no frame draws it on its way — the placeholder marks the
 *                        slot, and the release swaps them with no frame drawing a pane
 *                        off where it lands
 *   copy-takes-slot-shape  a tall pane carried below a wide one: at rest with the zone shown,
 *                        the copy is the placeholder's size, its centre on the pointer; over
 *                        its own place (no zone) it is its own size; each change of size is
 *                        drawn one way, never past where it goes; nothing under the controls
 *   preview-panes-take-shape  at rest with a zone shown, every pane is drawn at the rect the
 *                        drop then gives it, its transcript held to its top — centred across
 *                        where it grows, held left where it shrinks — and its composer to its
 *                        foot; its title and
 *                        the copy's are never drawn stretched, any frame
 *
 * `--shots <dir>` saves, from copy-takes-slot-shape, the copy below a wide
 * pane, beside it, and over its own place.
 *
 * `--reduced-motion` runs every check with the system's reduced motion on.
 */
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import {
  attempt,
  CannotRun,
  chosen,
  devServerOnlySteps,
  recordIfLeftOut,
} from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { safeArea, safeAreaInit, summarize } from "./lib/safe-area.mjs"
import { content, css, keys, modules, storage, zoneSaid } from "./lib/selectors.mjs"
import {
  dragResidue,
  frames,
  hideColumns,
  lift,
  modelRule,
  openPanes,
  order,
  panes,
  recordFrames,
  recordZones,
  residueFailures,
  settled,
  state,
  zoneSays,
} from "./lib/workspace.mjs"

const tolerance = 2

/**
 * How long the copy's glide to its centre may take: `--desktop-base`, read
 * where the drag reads it (the workspace, under its surface), and a frame.
 */
async function glideOf(page) {
  const token = await page.evaluate(
    (sel) =>
      getComputedStyle(document.querySelector(sel))
        .getPropertyValue("--desktop-base")
        .trim(),
    css.workspace,
  )
  const ms = token.endsWith("ms")
    ? Number.parseFloat(token)
    : token.endsWith("s")
      ? Number.parseFloat(token) * 1000
      : Number.NaN
  if (!Number.isFinite(ms)) throw new CannotRun(`--desktop-base reads "${token}"`)
  return ms + 40
}

function followsPointer(frames, liftedAt, glideMs) {
  const carried = frames.filter(
    (f) => f.ghost && f.pointer && f.t - liftedAt > glideMs && f.pointer.t > liftedAt,
  )
  if (carried.length < 5) return [`only ${carried.length} frames carried the copy`]
  const off = carried.filter(
    (f) =>
      Math.abs(f.pointer.x - (f.ghost.x + f.ghost.w / 2)) > tolerance ||
      Math.abs(f.pointer.y - (f.ghost.y + f.ghost.h / 2)) > tolerance,
  )
  return off.length
    ? [
        `the copy's centre left the pointer in ${off.length}/${carried.length} frames (e.g. Δ ${Math.round(off[0].pointer.x - off[0].ghost.x - off[0].ghost.w / 2)},${Math.round(off[0].pointer.y - off[0].ghost.y - off[0].ghost.h / 2)})`,
      ]
    : []
}

function insideGrid(frames) {
  const out = []
  for (const f of frames) {
    if (!f.grid) continue
    for (const p of f.panes)
      if (
        p.x < f.grid.x - tolerance ||
        p.y < f.grid.y - tolerance ||
        p.x + p.w > f.grid.x + f.grid.w + tolerance ||
        p.y + p.h > f.grid.y + f.grid.h + tolerance
      )
        out.push(
          `pane ${p.key} at ${Math.round(p.x)},${Math.round(p.y)} ${Math.round(p.w)}×${Math.round(p.h)}`,
        )
  }
  return out.length ? [`${out.length} pane-frames outside the grid, e.g. ${out[0]}`] : []
}

function insideWindow(frames) {
  const out = []
  for (const f of frames)
    for (const p of f.panes)
      if (
        p.x < -tolerance ||
        p.y < -tolerance ||
        p.x + p.w > f.viewport.w + tolerance ||
        p.y + p.h > f.viewport.h + tolerance
      )
        out.push(
          `pane ${p.key} at ${Math.round(p.x)},${Math.round(p.y)} ${Math.round(p.w)}×${Math.round(p.h)} in ${f.viewport.w}×${f.viewport.h}`,
        )
  return out.length
    ? [`${out.length} pane-frames outside the window, e.g. ${out[0]}`]
    : []
}

function oneWay(frames) {
  const reversals = []
  let segment = []
  const flush = () => {
    const keys = new Set(segment.flatMap((f) => f.panes.map((p) => p.key)))
    for (const key of keys)
      for (const axis of ["x", "y"]) {
        const values = segment
          .map((f) => f.panes.find((p) => p.key === key)?.[axis])
          .filter((v) => v !== undefined)
        let direction = 0
        for (let i = 1; i < values.length; i++) {
          const d = values[i] - values[i - 1]
          if (Math.abs(d) < 1) continue
          const s = Math.sign(d)
          if (direction && s !== direction) {
            reversals.push(
              `pane ${key} reversed on ${axis} within zone "${segment[0].zone}"`,
            )
            break
          }
          direction = s
        }
      }
    segment = []
  }
  for (const f of frames) {
    if (segment.length && f.zone !== segment[0].zone) flush()
    segment.push(f)
  }
  flush()
  return [...new Set(reversals)]
}

async function fourPanes(page, layout) {
  await hideColumns(page, layout)
  await openPanes(page, 4)
  return panes(page)
}

/** Three panes side by side: tall, narrow columns. */
async function threeColumns(page, layout) {
  await hideColumns(page, layout)
  await openPanes(page, 3)
  const list = await panes(page)
  const tops = new Set(list.map((p) => Math.round(p.y)))
  if (tops.size !== 1)
    throw new CannotRun(
      `expected three side-by-side panes, got tops ${[...tops].join(",")}`,
    )
  return list.sort((a, b) => a.x - b.x)
}

/** Lets go and waits for the copy's flight to end. */
async function letGo(page, { escape = false } = {}) {
  if (escape) await page.keyboard.press(keys.escape)
  await page.mouse.up()
  await settled(page)
}

const liftedNow = (page) => page.evaluate(() => performance.now())

/**
 * Records, every frame until `stop()`: the copy's painted rect, the
 * placeholder's, the pointer, the zone said, each pane's painted rect and
 * its transcript's and composer's, and
 * the scale each title — every pane's and the copy's — is drawn at, across
 * and down, by every transform above it.
 */
async function recordShapes(page) {
  await page.evaluate((sel) => {
    window.__shapes = []
    window.__shapesOn = true
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
    // row agree, and the agreed read is judged; it may be a moment later
    // than the frame drawn, which matters only to a stretch that comes and
    // goes within a frame. What this does not hide, as observed in #365 and
    // re-run there whenever this changes: a counter-scale started a frame
    // off still fails `copy-takes-slot-shape` in both engines. A chain that
    // never settles in `steadyReads` reads is marked `unsteady`, and every
    // check that judges a title fails it (`unsteadily`): never passed for
    // being unreadable.
    const steadyReads = 6
    const agree = (a, b) =>
      Math.abs(a.sx - b.sx) <= 0.002 && Math.abs(a.sy - b.sy) <= 0.002
    const drawnAt = (title) => {
      const reads = []
      let began = performance.now()
      let read = chain(title)
      let readMs = performance.now() - began
      reads.push([read.sx, read.sy, readMs])
      while (reads.length < steadyReads) {
        began = performance.now()
        const next = chain(title)
        readMs = performance.now() - began
        reads.push([next.sx, next.sy, readMs])
        const steady = agree(next, read)
        read = next
        if (steady) return { sx: read.sx, sy: read.sy, readMs, reads: reads.length }
      }
      return { sx: read.sx, sy: read.sy, readMs, reads: reads.length, unsteady: reads }
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
      const ghost = document.querySelector(sel.dragGhost)
      const placeholder = document.querySelector(sel.dragPlaceholder)
      const shown = ghost && ghost.checkVisibility({ checkOpacity: true })
      const copyTitle = ghost?.querySelector(sel.titleText)
      window.__shapes.push({
        t,
        pointer: window.__verifyPointer ?? null,
        zone: document.querySelector(sel.dropAnnouncer)?.textContent ?? "",
        ghost: shown ? rect(ghost) : null,
        placeholder: placeholder ? rect(placeholder) : null,
        panes: [...document.querySelectorAll(sel.pane)]
          .filter((e) => !e.closest(sel.dragGhost))
          .map((e) => {
            // Its transcript and composer, drawn.
            const transcript = e.querySelector(sel.transcript)
            const dock = e.querySelector(sel.dock)
            return {
              key: e.dataset.paneKey,
              ...rect(e),
              transcript: transcript ? rect(transcript) : null,
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
      if (window.__shapesOn) requestAnimationFrame(tick)
    }
    requestAnimationFrame(tick)
  }, css)
  return {
    stop: () =>
      page.evaluate(() => {
        window.__shapesOn = false
        return window.__shapes
      }),
    now: () => page.evaluate(() => window.__shapes.at(-1)),
  }
}

/** How far apart two numbers may be and still be the same length on screen. */
const near = (a, b) => Math.abs(a - b) <= tolerance

const px = (r) =>
  `${Math.round(r.w)}×${Math.round(r.h)} at ${Math.round(r.x)},${Math.round(r.y)}`

/**
 * Where the copy's size changed, one frame after another: the copy's width
 * and height, each, move one way and never past the sizes they start and end
 * at (no overshoot). Checked from `from` to `to` (frame times).
 */
function oneWaySize(frames, from, to, label) {
  const sizes = frames.filter((f) => f.ghost && f.t >= from && f.t <= to)
  if (sizes.length < 3) return [`${label}: only ${sizes.length} frames of the copy`]
  const out = []
  for (const axis of ["w", "h"]) {
    const values = sizes.map((f) => f.ghost[axis])
    const low = Math.min(values[0], values.at(-1)) - 1
    const high = Math.max(values[0], values.at(-1)) + 1
    const past = values.find((v) => v < low || v > high)
    if (past !== undefined)
      out.push(
        `${label}: the copy's ${axis === "w" ? "width" : "height"} went past ${Math.round(values[0])} → ${Math.round(values.at(-1))} (${Math.round(past)})`,
      )
    let direction = 0
    for (let i = 1; i < values.length; i++) {
      const d = values[i] - values[i - 1]
      if (Math.abs(d) < 0.5) continue
      if (direction && Math.sign(d) !== direction) {
        out.push(
          `${label}: the copy's ${axis === "w" ? "width" : "height"} turned back (${values
            .slice(Math.max(0, i - 2), i + 1)
            .map(Math.round)
            .join(" → ")})`,
        )
        break
      }
      direction = Math.sign(d)
    }
  }
  return out
}

/**
 * A title whose transforms never read the same twice in a row in one frame
 * (`recordShapes`): what it is drawn at is unknown, so it is a failure of
 * its own — never judged, never passed.
 */
function unsteadily(title, where) {
  return `${where}: ${title.of}'s title never read the same twice in a row (${JSON.stringify(title.unsteady)})`
}

/** Titles drawn stretched — across and down scaled apart — or not readable, in any frame. */
function stretched(frames, label) {
  const bad = []
  const unread = []
  for (const f of frames)
    for (const title of f.titles)
      if (title.unsteady) unread.push(unsteadily(title, `${Math.round(f.t)}ms`))
      else if (Math.abs(title.sx - title.sy) > 0.02)
        bad.push(
          `${title.of} ${title.sx.toFixed(3)}×${title.sy.toFixed(3)} at ${Math.round(f.t)}ms (read over ${title.readMs.toFixed(1)}ms), zone "${f.zone}"`,
        )
  return [
    ...(bad.length
      ? [`${label}: titles drawn stretched in ${bad.length} title-frames, e.g. ${bad[0]}`]
      : []),
    ...(unread.length
      ? [
          `${label}: titles not readable in ${unread.length} title-frames, e.g. ${unread[0]}`,
        ]
      : []),
  ]
}

/** Two panes side by side, each tall: the left is carried, the right is aimed at. */
async function sideBySide(page, layout) {
  await hideColumns(page, layout)
  await openPanes(page, 2)
  await settled(page)
  const [own, other] = (await panes(page)).sort((a, b) => a.x - b.x)
  return { own, other }
}

/** Heads down the pane `to` into its lower edge, where the drop is below it. */
async function headBelow(page, to) {
  await page.mouse.move(to.x + to.w / 2, to.y + to.h * 0.5, { steps: 10 })
  await page.mouse.move(to.x + to.w / 2, to.y + to.h * 0.93, { steps: 12 })
  return { x: to.x + to.w / 2, y: to.y + to.h * 0.93 }
}

/** Waits past the zone's rest and the copy's change of shape: the pointer still. */
async function atRest(page) {
  // `restAfter` and `--desktop-base` both pass: pacing, not a wait for state.
  await page.waitForTimeout(450)
  await frames(page, 2)
}

/**
 * A change mid-drag (`act`): the drag ends at once — nothing lifted right
 * after — every pane stays inside the window in every frame, nothing is
 * painted under the window's controls, and the release after drops nothing.
 */
async function changeMidDrag(page, layout, name, act, expectAfter) {
  const list = await threeColumns(page, layout)
  const target = list[2]
  const sampler = safeArea(page)
  const before = await order(page)
  await lift(page, 0)
  await page.mouse.move(target.x + target.w * 0.5, target.y + target.h * 0.5, {
    steps: 10,
  })
  if (!(await zoneSays(page, zoneSaid.any)))
    throw new CannotRun(
      `${name}: no zone was offered over the far pane before the change`,
    )
  const recorder = await recordFrames(page)
  const failures = []
  await sampler.watch(
    name,
    async () => {
      await act()
      // Ended at once: by the next frames nothing of the drag is on the page.
      await frames(page, 2)
      const residue = await dragResidue(page)
      for (const key of ["copies", "placeholders", "shields", "lifted", "dragging"])
        if (residue[key]) failures.push(`${name}: ${residue[key]} ${key} right after it`)
      await page.mouse.move(target.x + target.w * 0.5 + 3, target.y + target.h * 0.5 + 3)
      await page.mouse.up()
      await settled(page)
    },
    2000,
  )
  const recorded = await recorder.stop()
  failures.push(...insideWindow(recorded).map((f) => `${name}: ${f}`))
  failures.push(...summarize(await sampler.take()).map((f) => `${name} safe-area: ${f}`))
  failures.push(...residueFailures(await dragResidue(page)).map((f) => `${name}: ${f}`))
  const problem = expectAfter(before, await order(page), await state(page))
  if (problem) failures.push(`${name}: ${problem}`)
  return failures
}

const checks = {
  "sweep-across-zones": async (page, layout) => {
    const list = await fourPanes(page, layout)
    const grid = await page.locator(css.paneGrid).first().boundingBox()
    const glideMs = await glideOf(page)
    const recorder = await recordFrames(page)
    await lift(page, 0)
    const liftedAt = await liftedNow(page)
    for (const [fx, fy] of [
      [0.9, 0.25],
      [0.75, 0.75],
      [0.25, 0.9],
      [0.6, 0.1],
      [0.95, 0.5],
    ]) {
      await page.mouse.move(grid.x + grid.width * fx, grid.y + grid.height * fy, {
        steps: 20,
      })
      // Resting on each point: pacing the drag, not a wait for state.
      await page.waitForTimeout(250)
    }
    const during = await recorder.stop()
    await letGo(page, { escape: true })
    return {
      panes: list.length,
      frames: during.length,
      failures: [
        ...followsPointer(during, liftedAt, glideMs).map((f) => `follows-pointer: ${f}`),
        ...insideGrid(during).map((f) => `inside-grid: ${f}`),
        ...insideWindow(during).map((f) => `inside-window: ${f}`),
        ...oneWay(during).map((f) => `one-way: ${f}`),
        ...(during.some((f) => f.selection)
          ? [
              `no-selection: "${during.find((f) => f.selection).selection}" selected during the drag`,
            ]
          : []),
      ],
    }
  },
  "sideways-top": async (page, layout) => {
    const [, middle] = await threeColumns(page, layout)
    const failures = []
    for (const dy of [30, 50, 90]) {
      await lift(page, 2)
      await page.mouse.move(middle.x + middle.w + 30, middle.y + dy, { steps: 10 })
      const zones = await recordZones(page)
      await page.mouse.move(middle.x + middle.w * 0.45, middle.y + dy, { steps: 30 })
      await frames(page, 2)
      const said = await zones.take()
      if (!said.length)
        failures.push(`sweeping left at top+${dy}px announced no zone at all`)
      if (said.some((z) => zoneSaid.vertical.test(z)))
        failures.push(
          `sweeping left at top+${dy}px picked a vertical zone: ${said.join(" → ")}`,
        )
      await letGo(page, { escape: true })
    }
    return { failures }
  },
  "boundary-jitter": async (page, layout) => {
    const list = await fourPanes(page, layout)
    const target = list[1]
    // The left side's reach at rest, as the model decides it (`edgeReach`).
    const inset = await modelRule(
      page,
      modules.drop,
      "edgeReach",
      { width: target.w, height: target.h },
      "left",
    )
    const x = target.x + inset
    await lift(page, 0)
    await page.mouse.move(x, target.y + target.h / 2, { steps: 10 })
    // At rest on the boundary before the jitter: pacing (`restAfter` is 150 ms).
    await page.waitForTimeout(200)
    const zones = await recordZones(page)
    for (let i = 0; i < 40; i++)
      await page.mouse.move(x + (i % 2 ? 6 : -6), target.y + target.h / 2)
    await frames(page, 2)
    const said = await zones.take()
    const current = await page.locator(css.dropAnnouncer).first().textContent()
    await letGo(page, { escape: true })
    if (!said.length && !current)
      return { failures: ["no zone is announced at the boundary"] }
    return {
      failures:
        said.length > 1
          ? [`the zone flickered ${said.length} times: ${said.slice(0, 6).join(" → ")}`]
          : [],
    }
  },
  "rest-settles": async (page, layout) => {
    const list = await threeColumns(page, layout)
    const target = list[2]
    const failures = []
    // Three places: near the top, off-centre, the middle — each approached fast.
    for (const [fx, fy] of [
      [0.5, 0.2],
      [0.6, 0.2],
      [0.5, 0.5],
    ]) {
      await lift(page, 0)
      const x = target.x + target.w * fx
      const y = target.y + target.h * fy
      await page.mouse.move(x, y, { steps: 4 })
      // Still for twice `restAfter`: the check's own condition, not a wait for state.
      await page.waitForTimeout(300)
      const rested = await page.locator(css.dropAnnouncer).first().textContent()
      await page.mouse.move(x + 1, y)
      // After the nudge, as long again: a zone change would show within it.
      await page.waitForTimeout(200)
      const nudged = await page.locator(css.dropAnnouncer).first().textContent()
      if (!rested) failures.push(`${fx},${fy}: no zone after resting 300 ms`)
      if (nudged !== rested)
        failures.push(
          `${fx},${fy}: a 1px nudge after resting changed "${rested}" → "${nudged}"`,
        )
      await letGo(page, { escape: true })
    }
    return { failures }
  },
  "swap-in-tall-pane": async (page, layout) => {
    const list = await threeColumns(page, layout)
    const target = list[2]
    await lift(page, 0)
    await page.mouse.move(target.x + target.w / 2, target.y + target.h / 2, { steps: 15 })
    const swap = await zoneSays(page, zoneSaid.swap)
    const said = await page.locator(css.dropAnnouncer).first().textContent()
    await letGo(page, { escape: true })
    return {
      pane: `${Math.round(target.w)}×${Math.round(target.h)}`,
      failures: swap ? [] : [`the middle of a tall pane offers "${said}", not a swap`],
    }
  },
  "escape-cancels": async (page, layout) => {
    const list = await fourPanes(page, layout)
    const before = (await order(page)).join(",")
    await lift(page, 0)
    await page.mouse.move(list[2].x + 30, list[2].y + list[2].h / 2, { steps: 10 })
    await page.keyboard.press(keys.escape)
    await settled(page)
    const residue = await dragResidue(page)
    await page.mouse.move(list[3].x + 30, list[3].y + list[3].h / 2, { steps: 5 })
    await page.mouse.up()
    await settled(page)
    const after = (await order(page)).join(",")
    const failures = residueFailures(residue).map((f) => `after Escape: ${f}`)
    if (after !== before)
      failures.push(`layout changed after Escape then release: ${before} → ${after}`)
    return { failures }
  },
  "outside-cancels": async (page, layout) => {
    const list = await fourPanes(page, layout)
    const size = page.viewportSize()
    const before = (await order(page)).join(",")
    await lift(page, 0)
    await page.mouse.move(list[1].x + 30, list[1].y + list[1].h / 2, { steps: 8 })
    await page.evaluate(() =>
      addEventListener(
        "pointermove",
        (e) => (window.__verifyLast = { x: e.clientX, y: e.clientY }),
        { capture: true },
      ),
    )
    await page.mouse.move(size.width - 1, size.height - 1, { steps: 8 })
    await page.mouse.move(size.width + 60, size.height + 50, { steps: 3 })
    const last = await page.evaluate(() => window.__verifyLast)
    if (!last || (last.x < size.width && last.y < size.height)) {
      await letGo(page, { escape: true })
      throw new CannotRun(
        `this engine delivered no pointer event outside the viewport (last at ${last?.x},${last?.y}); ` +
          "check a release outside the window by hand in the app",
      )
    }
    await page.mouse.up()
    await settled(page)
    const failures = residueFailures(await dragResidue(page))
    const selection = await page.evaluate(() => String(getSelection() ?? "").trim())
    const now = (await order(page)).join(",")
    if (now !== before)
      failures.push(`layout changed after a release outside: ${before} → ${now}`)
    if (selection)
      failures.push(`no-selection: "${selection.slice(0, 40)}" selected after the drag`)
    return { failures }
  },
  "lost-capture-then-move": async (page, layout) => {
    const list = await threeColumns(page, layout)
    const target = list[2]
    const before = (await order(page)).join(",")
    await lift(page, 0)
    await page.mouse.move(target.x + target.w / 2, target.y + target.h / 2, { steps: 10 })
    if (!(await zoneSays(page, zoneSaid.any)))
      throw new CannotRun("no zone was offered before the pointer was lost")
    const lost = await page.evaluate((shield) => {
      const element = document.querySelector(shield)
      element?.dispatchEvent(new PointerEvent("lostpointercapture"))
      return !!element
    }, css.dragShield)
    if (!lost)
      throw new CannotRun(`no drag shield (${css.dragShield}) to lose the pointer from`)
    await settled(page)
    // Lost, then the pointer moves on and is released over a zone.
    await page.mouse.move(target.x + target.w / 2 + 20, target.y + target.h / 2, {
      steps: 5,
    })
    const resumed = await dragResidue(page)
    await page.mouse.up()
    await settled(page)
    const failures = []
    if (resumed.copies || resumed.dragging)
      failures.push("a move after the pointer was lost started the drag again")
    failures.push(...residueFailures(await dragResidue(page)))
    const after = (await order(page)).join(",")
    if (after !== before)
      failures.push(`the release after the loss dropped: ${before} → ${after}`)
    return { failures }
  },
  "resize-mid-drag": async (page, layout) => {
    const size = page.viewportSize()
    const failures = await changeMidDrag(
      page,
      layout,
      "resize",
      () => page.setViewportSize({ width: size.width - 200, height: size.height - 100 }),
      (before, after) =>
        after.length <= before.length && after.every((key) => before.includes(key))
          ? null
          : `the release after a resize dropped: ${before} → ${after}`,
    )
    await page.setViewportSize(size)
    return { failures }
  },
  "command-mid-drag": async (page, layout, fresh) => {
    const failures = [
      ...(await changeMidDrag(
        page,
        layout,
        "⌘W",
        () => page.keyboard.press(keys.closePane),
        (b, a) =>
          a.length === b.length - 1
            ? null
            : `⌘W mid-drag: panes ${b} → ${a}, expected one fewer and no drop`,
      )),
    ]
    const next = await fresh()
    failures.push(
      ...(await changeMidDrag(
        next,
        layout,
        "⌘0",
        () => next.keyboard.press(keys.overview),
        (b, a, s) =>
          s.content === content.overview && a.join(",") === b.join(",")
            ? null
            : `⌘0 mid-drag: content ${s.content}, panes ${b} → ${a}; expected the overview and no drop`,
      )),
    )
    return { failures }
  },
  "overview-session-drop": {
    layouts: ["sidebar"],
    run: async (page) => {
      await openPanes(page, 2)
      await page.keyboard.press(keys.overview)
      await page.waitForFunction(
        ([sel, value]) => document.querySelector(sel)?.dataset.content === value,
        [css.workspace, content.overview],
      )
      const before = (await order(page)).join(",")
      const row = page.locator(`${css.sidebar} ${css.sessionRow}`).nth(3)
      const box = await row.boundingBox()
      if (!box) throw new CannotRun(`no session row in the sidebar (${css.sessionRow})`)
      await page.mouse.move(box.x + 40, box.y + box.height / 2)
      await page.mouse.down()
      for (let i = 1; i <= 6; i++)
        await page.mouse.move(box.x + 40 + i * 5, box.y + box.height / 2 + i * 2)
      const grid = await page.locator(css.paneGrid).first().boundingBox()
      const zones = await recordZones(page)
      await page.mouse.move(grid.x + grid.width * 0.7, grid.y + grid.height / 2, {
        steps: 20,
      })
      // Past `restAfter`, so a zone would have settled if one were offered.
      await page.waitForTimeout(300)
      const said = await zones.take()
      const placeholders = await page.locator(css.dragPlaceholder).count()
      await letGo(page)
      const failures = []
      if (said.length)
        failures.push(`a zone was offered under the overview: ${said.join(" → ")}`)
      if (placeholders) failures.push("a placeholder was drawn under the overview")
      const after = (await order(page)).join(",")
      if (after !== before)
        failures.push(
          `the release under the overview changed the panes: ${before} → ${after}`,
        )
      failures.push(...residueFailures(await dragResidue(page)))
      return { failures }
    },
  },
}

Object.assign(checks, {
  flick: async (page, layout) => {
    const list = await threeColumns(page, layout)
    const far = list[2]
    const before = (await order(page)).join(",")
    const title = await page.evaluate((sel) => {
      const r = document.querySelector(sel).getBoundingClientRect()
      return { x: r.left + Math.min(r.width / 2, 40), y: r.top + r.height / 2 }
    }, css.paneTitle)
    const to = { x: far.x + far.w / 2, y: far.y + far.h / 2 }
    // Down, across and up in one task: no frame between them.
    await page.evaluate(
      ([sel, from, to]) => {
        const at = (type, p, target = window) =>
          target.dispatchEvent(
            new PointerEvent(type, {
              clientX: p.x,
              clientY: p.y,
              pointerId: 1,
              isPrimary: true,
              button: 0,
              buttons: type === "pointerup" ? 0 : 1,
              bubbles: true,
              cancelable: true,
            }),
          )
        const title =
          document.elementFromPoint(from.x, from.y) ?? document.querySelector(sel)
        at("pointerdown", from, title)
        at("pointermove", to)
        at("pointerup", to)
      },
      [css.paneTitle, title, to],
    )
    await settled(page)
    await frames(page, 4)
    const failures = residueFailures(await dragResidue(page)).map(
      (f) => `before any copy: ${f}`,
    )
    const once = (await order(page)).join(",")
    if (once !== before)
      failures.push(`a flick before any copy dropped: ${before} → ${once}`)
    // Lifted, then across and up in one task, before any preview is shown.
    await page.evaluate(() =>
      addEventListener("pointerdown", (e) => (window.__verifyPointerId = e.pointerId), {
        capture: true,
        once: true,
      }),
    )
    await lift(page, 0)
    await page.evaluate((to) => {
      const at = (type) =>
        dispatchEvent(
          new PointerEvent(type, {
            clientX: to.x,
            clientY: to.y,
            pointerId: window.__verifyPointerId,
            isPrimary: true,
            button: 0,
            buttons: type === "pointerup" ? 0 : 1,
            bubbles: true,
            cancelable: true,
          }),
        )
      at("pointermove")
      at("pointerup")
    }, to)
    await page.mouse.up()
    await settled(page)
    await frames(page, 4)
    failures.push(
      ...residueFailures(await dragResidue(page)).map((f) => `before any preview: ${f}`),
    )
    const after = (await order(page)).join(",")
    if (after !== before)
      failures.push(`a release before any preview dropped: ${before} → ${after}`)
    return { failures }
  },
  "chord-right-button": async (page, layout) => {
    const list = await threeColumns(page, layout)
    const target = list[2]
    const before = (await order(page)).join(",")
    await lift(page, 0)
    await page.mouse.move(target.x + target.w / 2, target.y + target.h / 2, { steps: 6 })
    if (!(await zoneSays(page, zoneSaid.any)))
      throw new CannotRun("no zone was offered before the chord")
    await page.mouse.down({ button: "right" })
    await settled(page)
    await frames(page, 2)
    const during = await dragResidue(page)
    await page.mouse.up({ button: "left" })
    await page.mouse.up({ button: "right" })
    await settled(page)
    // A menu the right button opened is its own; close it before counting.
    if (await page.locator(css.menu).count()) await page.keyboard.press(keys.escape)
    const failures = []
    if (during.copies || during.dragging)
      failures.push("the copy was still carried after the right button joined the left")
    failures.push(...residueFailures(await dragResidue(page)))
    const after = (await order(page)).join(",")
    if (after !== before)
      failures.push(`the chord's release dropped: ${before} → ${after}`)
    return { failures }
  },
  "peek-session-no-zone": {
    layouts: ["sidebar"],
    run: async (page) => {
      await openPanes(page, 2)
      await page.keyboard.press(keys.toggleSidebar)
      await settled(page)
      const size = page.viewportSize()
      await page.mouse.move(3, size.height / 2)
      const peeked = () =>
        page.evaluate(
          (sel) => "peek" in (document.querySelector(sel)?.dataset ?? {}),
          css.workspace,
        )
      await page.waitForFunction(
        (sel) => "peek" in (document.querySelector(sel)?.dataset ?? {}),
        css.workspace,
        { timeout: 2000 },
      )
      await settled(page)
      const rows = await page.evaluate(
        ([sidebar, row]) =>
          [...document.querySelectorAll(`${sidebar} ${row}`)]
            .map((e) => e.getBoundingClientRect())
            .filter((r) => r.width > 0 && r.left >= 0)
            .map((r) => ({ x: r.left, y: r.top, w: r.width, h: r.height })),
        [css.sidebar, css.sessionRow],
      )
      if (!rows.length)
        throw new CannotRun(`no session row in the revealed sidebar (${css.sessionRow})`)
      const row = rows[Math.min(3, rows.length - 1)]
      const before = (await order(page)).join(",")
      const zones = await recordZones(page)
      await page.mouse.move(row.x + 20, row.y + row.h / 2)
      await page.mouse.down()
      await page.waitForSelector(css.dragGhost, { state: "attached", timeout: 2000 })
      for (let i = 1; i <= 8; i++)
        await page.mouse.move(row.x + 20 + i * 4, row.y + row.h / 2 + i)
      // Past `restAfter`, and past the reveal's own hide delay: pacing, not a wait for state.
      await page.waitForTimeout(500)
      const said = await zones.take()
      const placeholders = await page.locator(css.dragPlaceholder).count()
      const carried = (await dragResidue(page)).copies
      const stayed = await peeked()
      await page.mouse.up()
      await settled(page)
      await frames(page, 4)
      const failures = []
      if (!carried) failures.push("nothing was carried from the revealed sidebar")
      if (said.length)
        failures.push(`a zone was offered over the sidebar: ${said.join(" → ")}`)
      if (placeholders) failures.push("a placeholder was drawn under the sidebar")
      if (!stayed)
        failures.push("the revealed sidebar hid while a session was carried in it")
      const after = (await order(page)).join(",")
      if (after !== before)
        failures.push(
          `the release over the sidebar changed the panes: ${before} → ${after}`,
        )
      failures.push(...residueFailures(await dragResidue(page)))
      return { failures }
    },
  },
  "docked-columns-no-zone": {
    sizes: ["1440x900"],
    run: async (page, layout) => {
      await openPanes(page, 2)
      await settled(page)
      const columns = await page.evaluate(
        ([sidebar, list]) =>
          [sidebar, list].flatMap((sel) => {
            const element = document.querySelector(sel)
            const r = element?.getBoundingClientRect()
            return r && r.width > 40 && r.right > 0 && r.left < innerWidth
              ? [{ sel, x: r.left, y: r.top, w: r.width, h: r.height }]
              : []
          }),
        [css.sidebar, css.sessionList],
      )
      const want = layout === "columns" ? 2 : 1
      if (columns.length < want)
        throw new CannotRun(
          `expected ${want} docked side columns, found ${columns.map((c) => c.sel).join(", ") || "none"}`,
        )
      const before = (await order(page)).join(",")
      await lift(page, 0)
      const failures = []
      for (const column of columns) {
        await page.mouse.move(column.x + column.w / 2, column.y + column.h / 2, {
          steps: 12,
        })
        // Past `restAfter`: pacing, then what the page shows there.
        await page.waitForTimeout(250)
        await frames(page, 2)
        const said = await page.locator(css.dropAnnouncer).first().textContent()
        const placeholders = await page.locator(css.dragPlaceholder).count()
        if (said) failures.push(`over the docked ${column.sel}, a zone: "${said}"`)
        if (placeholders) failures.push(`over the docked ${column.sel}, a placeholder`)
      }
      await page.mouse.up()
      await settled(page)
      await frames(page, 4)
      const after = (await order(page)).join(",")
      if (after !== before)
        failures.push(
          `the release over a side column changed the panes: ${before} → ${after}`,
        )
      failures.push(...residueFailures(await dragResidue(page)))
      return { columns: columns.map((c) => c.sel), failures }
    },
  },
  "peek-press-while-hiding": async (page, layout) => {
    await hideColumns(page, layout)
    await openPanes(page, 2)
    await settled(page)
    const size = page.viewportSize()
    const peekShown = (sel) => "peek" in (document.querySelector(sel)?.dataset ?? {})
    await page.mouse.move(3, size.height / 2)
    await page.waitForFunction(peekShown, css.workspace, { timeout: 2000 })
    await settled(page)
    const peek = await page.locator(css.sidebar).first().boundingBox()
    if (!peek) throw new CannotRun(`the revealed sidebar (${css.sidebar}) has no box`)
    // Every frame from here on: is the reveal still there?
    await page.evaluate((sel) => {
      window.__peekGone = 0
      window.__peekWatch = true
      const tick = () => {
        if (!("peek" in (document.querySelector(sel)?.dataset ?? {}))) window.__peekGone++
        if (window.__peekWatch) requestAnimationFrame(tick)
      }
      requestAnimationFrame(tick)
    }, css.workspace)
    const left = await page.evaluate(() => performance.now())
    // Out of the reveal to the right pane's title, and pressed at once:
    // inside the reveal's 350 ms hide.
    await lift(page, 1)
    const pressedAfter = (await page.evaluate(() => performance.now())) - left
    const failures = []
    // Carried about the right pane past the hide's delay, then over the reveal.
    const panesNow = (await panes(page)).sort((a, b) => a.x - b.x)
    const right = panesNow.at(-1)
    await page.mouse.move(right.x + right.w / 2, right.y + right.h / 2, { steps: 10 })
    await page.waitForTimeout(400)
    await page.mouse.move(peek.x + peek.width / 2, peek.y + peek.height / 2, {
      steps: 12,
    })
    await page.waitForTimeout(250)
    await frames(page, 2)
    const overPeek = await page.locator(css.dropAnnouncer).first().textContent()
    if (overPeek) failures.push(`over the revealed sidebar, a zone: "${overPeek}"`)
    // Just past its edge, over the first pane: a target, not a dead area.
    await page.mouse.move(peek.x + peek.width + 40, peek.y + peek.height / 2, {
      steps: 6,
    })
    const pastPeek = await zoneSays(page, zoneSaid.any, 1500)
    if (!pastPeek)
      failures.push("just past the revealed sidebar's edge, no zone: a dead area")
    const gone = await page.evaluate(() => {
      window.__peekWatch = false
      return window.__peekGone
    })
    if (gone) failures.push(`the reveal was gone in ${gone} frames of the drag`)
    await page.keyboard.press(keys.escape)
    await page.mouse.up()
    await settled(page)
    // Released away from it: now it hides, on its own delay.
    const hid = await page
      .waitForFunction(
        (sel) => !("peek" in (document.querySelector(sel)?.dataset ?? {})),
        css.workspace,
        {
          timeout: 2000,
        },
      )
      .then(() => true)
      .catch(() => false)
    if (!hid) failures.push("released away from it, the reveal never hid")
    failures.push(...residueFailures(await dragResidue(page)))
    return { pressedAfterLeaving: Math.round(pressedAfter), failures }
  },
  "reduced-motion": async (page, layout, fresh) => {
    // Motion set to Reduced in the window's own settings, as the app window
    // that showed no swap was (#286); the system's setting takes the same path.
    const still = await fresh({ prefs: { [storage.motion]: "reduced" } })
    const list = await threeColumns(still, layout)
    const [own, next] = list
    const shapes = await recordShapes(still)
    await lift(still, 0)
    // Over the middle of the pane beside it: a swap.
    await still.mouse.move(next.x + next.w / 2, next.y + next.h / 2, { steps: 20 })
    const said = await zoneSays(still, zoneSaid.swap)
    await still.waitForTimeout(300)
    const placeholders = await still.locator(css.dragPlaceholder).count()
    const recorded = await shapes.stop()
    const before = list.map((p) => p.key)
    const landing = await recordShapes(still)
    await still.mouse.up()
    await settled(still)
    // Nothing flies with less motion: the drop's marks go two frames on.
    await frames(still, 4)
    const landed = await landing.stop()
    const after = await order(still)
    const failures = []
    if (!said) throw new CannotRun("no swap was offered over the pane beside it")
    // The pane beside it is drawn where the drop puts it — in the carried
    // pane's place.
    const drawn = recorded.at(-1)?.panes.find((p) => p.key === next.key) ?? next
    if (!near(drawn.x, own.x) || !near(drawn.w, own.w))
      failures.push(
        `with a swap shown, the pane beside is drawn ${px(drawn)}, not in the carried pane's place ${px(own)}`,
      )
    // And gets there at once: while the zone said stays the same, no pane
    // is drawn anywhere new from one frame to the next.
    const glided = recorded.slice(1).flatMap((f, i) => {
      const prior = recorded[i]
      if (f.zone !== prior.zone) return []
      return f.panes.filter((p) => {
        const was = prior.panes.find((q) => q.key === p.key)
        return was && (!near(p.x, was.x) || !near(p.y, was.y) || !near(p.w, was.w))
      })
    })
    if (glided.length)
      failures.push(
        `with less motion, a pane glided: ${px(glided[0])} (${glided.length} frames)`,
      )
    // Released, each pane is where the preview drew it, every frame: the
    // preview goes as the drop lays them out, never drawing them moved again.
    const final = landed.at(-1)?.panes ?? []
    const jumped = landed.flatMap((f) =>
      f.panes.filter((p) => {
        const end = final.find((q) => q.key === p.key)
        return end && (!near(p.x, end.x) || !near(p.w, end.w))
      }),
    )
    if (jumped.length)
      failures.push(
        `released, a pane was drawn off where it landed: ${px(jumped[0])} (${jumped.length} frames)`,
      )
    if (!placeholders) failures.push("no placeholder marks where the drop would land")
    if (after.join(",") !== [before[1], before[0], ...before.slice(2)].join(","))
      failures.push(`released on the swap shown: ${before} became ${after}`)
    failures.push(...residueFailures(await dragResidue(still)))
    return { failures }
  },
  "copy-takes-slot-shape": async (page, layout, fresh, run) => {
    const { own, other } = await sideBySide(page, layout)
    const failures = []
    const sampler = safeArea(page)
    const shapes = await recordShapes(page)
    const shot = async (name) => {
      if (!run.shots) return
      mkdirSync(run.shots, { recursive: true })
      await page.screenshot({ path: join(run.shots, `${run.tag}-${name}.png`) })
    }
    /** At rest now: the copy against the slot it should have and the pointer. */
    const restsAs = async (label, slot) => {
      const f = await shapes.now()
      if (!f?.ghost || !f.pointer) return [`${label}: no copy drawn at rest`]
      const out = []
      if (!near(f.ghost.w, slot.w) || !near(f.ghost.h, slot.h))
        out.push(
          `${label}: the copy is ${px(f.ghost)}, not ${Math.round(slot.w)}×${Math.round(slot.h)}`,
        )
      const title = f.titles.find((t) => t.of === "copy")
      if (title?.unsteady) out.push(unsteadily(title, `${label}: at rest`))
      else if (title && (Math.abs(title.sx - 1) > 0.02 || Math.abs(title.sy - 1) > 0.02))
        out.push(
          `${label}: at rest the copy's title is drawn at ${title.sx.toFixed(3)}×${title.sy.toFixed(3)}, not its own size`,
        )
      const cx = f.ghost.x + f.ghost.w / 2
      const cy = f.ghost.y + f.ghost.h / 2
      if (!near(cx, f.pointer.x) || !near(cy, f.pointer.y))
        out.push(
          `${label}: the copy's centre is ${Math.round(cx - f.pointer.x)},${Math.round(cy - f.pointer.y)} from the pointer`,
        )
      return out
    }
    let below = null
    /** Frames in which the copy's title was measured: a stretch check over none holds nothing. */
    let copyTitled = 0
    const copyFrames = (list) =>
      list.filter((f) => f.titles.some((title) => title.of === "copy")).length
    await sampler.watch(
      "carry below, beside, and back",
      async () => {
        await lift(page, 0)
        const t0 = (await shapes.now())?.t ?? 0
        await headBelow(page, other)
        if (!(await zoneSays(page, /below/)))
          throw new CannotRun(
            `heading down the other pane offered "${await page.locator(css.dropAnnouncer).first().textContent()}", not below it`,
          )
        await atRest(page)
        const rest = await shapes.now()
        below = rest.placeholder
        if (!below) failures.push("below: no placeholder at rest")
        else failures.push(...(await restsAs("below", below)))
        if (below && below.w <= below.h)
          throw new CannotRun(`the slot below is ${px(below)}: not wide and short`)
        await shot("below")
        // The copy grew wide and short: one way, never past it.
        const toBelow = await shapes.stop()
        const zoneAt = toBelow.find((f) => /below/.test(f.zone))?.t ?? t0
        failures.push(...oneWaySize(toBelow, zoneAt, rest.t, "into the slot below"))
        failures.push(...stretched(toBelow, "into the slot below"))
        copyTitled += copyFrames(toBelow)
        // Beside it, to its right: a tall slot.
        const again = await recordShapes(page)
        await page.mouse.move(other.x + other.w * 0.93, other.y + other.h * 0.5, {
          steps: 12,
        })
        if (await zoneSays(page, /right of/)) {
          await atRest(page)
          const beside = (await again.now()).placeholder
          if (beside) failures.push(...(await restsAs("beside", beside)))
          await shot("right")
        } else
          failures.push("heading right in the other pane offered no zone to its right")
        // Over its own place: nothing offered, its own size.
        await page.mouse.move(own.x + own.w / 2, own.y + own.h / 2, { steps: 12 })
        const cleared = await zoneSays(page, "")
        await atRest(page)
        const back = await again.stop()
        if (!cleared) failures.push("over its own place, a zone was still said")
        failures.push(...(await restsAs("own place", own)))
        await shot("own-place")
        const leftAt = [...back].reverse().find((f) => f.zone !== "")?.t ?? 0
        failures.push(...oneWaySize(back, leftAt, back.at(-1).t, "back to its own size"))
        failures.push(...stretched(back, "beside and back"))
        copyTitled += copyFrames(back)
        await page.keyboard.press(keys.escape)
        await page.mouse.up()
      },
      3000,
    )
    await settled(page)
    if (!copyTitled)
      failures.push("the copy's title was never measured: no stretch was checked")
    failures.push(...summarize(await sampler.take()).map((f) => `safe-area: ${f}`))
    failures.push(...residueFailures(await dragResidue(page)))
    return {
      own: px(own),
      below: below ? px(below) : null,
      copyTitled,
      failures,
    }
  },
  "preview-panes-take-shape": async (page, layout) => {
    await hideColumns(page, layout)
    await openPanes(page, 3)
    await settled(page)
    const list = (await panes(page)).sort((a, b) => a.x - b.x)
    const target = list.at(-1)
    const shapes = await recordShapes(page)
    // Each pane's parts as laid out, before anything is previewed.
    await frames(page)
    const laid = (await shapes.now()).panes
    await lift(page, 0)
    await headBelow(page, target)
    if (!(await zoneSays(page, /below/)))
      throw new CannotRun("heading down the far pane offered no zone below it")
    await atRest(page)
    const shown = await shapes.now()
    const said = shown.zone
    const releasedAt = await page.evaluate(() => performance.now())
    await page.mouse.up()
    await settled(page)
    const during = await shapes.stop()
    const landed = await panes(page)
    const failures = []
    const moved = landed.filter((p) => {
      const was = list.find((q) => q.key === p.key)
      return was && (!near(was.w, p.w) || !near(was.h, p.h))
    })
    if (!moved.length)
      throw new CannotRun(`"${said}" changed no pane's size: nothing to take shape`)
    for (const p of landed) {
      const previewed = shown.panes.find((q) => q.key === p.key)
      if (!previewed) failures.push(`pane ${p.key} was not drawn in the preview`)
      else if (
        !near(previewed.x, p.x) ||
        !near(previewed.y, p.y) ||
        !near(previewed.w, p.w) ||
        !near(previewed.h, p.h)
      )
        failures.push(`pane ${p.key} previewed ${px(previewed)}, landed ${px(p)}`)
    }
    // Each pane's transcript keeps its top, centred across the shape it
    // would take where that grows, as a pane that wide centres it, and held
    // left where it shrinks, so its lines lose their ends, never their
    // starts; its composer keeps to the foot.
    for (const p of shown.panes) {
      const was = laid.find((q) => q.key === p.key)
      if (!was) continue
      if (p.transcript && was.transcript) {
        const dy = p.transcript.y - p.y - (was.transcript.y - was.y)
        const dx =
          p.w >= was.w - tolerance
            ? p.transcript.x + p.transcript.w / 2 - (p.x + p.w / 2)
            : p.transcript.x - p.x - (was.transcript.x - was.x)
        if (Math.abs(dy) > tolerance || Math.abs(dx) > tolerance)
          failures.push(
            `pane ${p.key}'s transcript is drawn ${Math.round(dx)}px across and ${Math.round(dy)}px down from where its previewed rect keeps it`,
          )
      }
      if (p.dock && was.dock) {
        const dy =
          p.y + p.h - (p.dock.y + p.dock.h) - (was.y + was.h - (was.dock.y + was.dock.h))
        if (Math.abs(dy) > tolerance)
          failures.push(
            `pane ${p.key}'s composer is drawn ${Math.round(dy)}px off its previewed rect's foot`,
          )
      }
    }
    failures.push(
      ...stretched(during, `"${said}"`).map(
        (f) => `${f} (released at ${Math.round(releasedAt)}ms)`,
      ),
    )
    failures.push(...residueFailures(await dragResidue(page)))
    return {
      zone: said,
      resized: moved.map((p) => `${p.key}: ${px(p)}`),
      failures,
    }
  },
  "copy-under-controls": async (page, layout) => {
    await hideColumns(page, layout)
    await openPanes(page, 2)
    const sampler = safeArea(page)
    // Where the corner pane's title starts in its pane: past the window's
    // controls, as a pane in the corner lays out its header.
    const titleIn = (box, title) =>
      page.evaluate(
        ([boxSelector, titleSelector]) => {
          const outer = document.querySelector(boxSelector)
          const inner = outer?.querySelector(titleSelector)
          return outer && inner
            ? inner.getBoundingClientRect().left - outer.getBoundingClientRect().left
            : null
        },
        [box, title],
      )
    // The pane measured, and lifted, must be the corner's, with both side
    // columns closed so its header steps past the controls — or the check
    // compares two plainly padded headers.
    const [corner, alone] = await page.evaluate(
      ([pane, cornerPane, panesAlone]) => [
        !!document.querySelector(pane)?.matches(cornerPane),
        !!document.querySelector(panesAlone),
      ],
      [css.pane, css.cornerPane, css.panesAlone],
    )
    if (!corner)
      throw new CannotRun(`the first pane is not ${css.cornerPane}: nothing to compare`)
    if (!alone)
      throw new CannotRun(
        `no ${css.panesAlone}: a side column is open, nothing to compare`,
      )
    const inPane = await titleIn(css.pane, css.titleText)
    let inCopy = null
    await sampler.watch(
      "carry the corner pane",
      async () => {
        await lift(page, 0)
        // Held near where it was grabbed while the copy glides to its centre.
        await page.waitForTimeout(300)
        inCopy = await titleIn(css.dragGhost, css.titleText)
        await page.keyboard.press(keys.escape)
        await page.mouse.up()
      },
      2000,
    )
    await settled(page)
    return {
      frames: await sampler.frames(),
      failures: [
        ...summarize(await sampler.take()),
        ...residueFailures(await dragResidue(page)),
        // The copy is the pane as it looks: its header keeps the corner's
        // start, never the plain padding of a pane beside another.
        ...(inPane === null || inCopy === null
          ? [`no title to measure (pane ${inPane}, copy ${inCopy})`]
          : Math.abs(inCopy - inPane) > 2
            ? [`the copy's title starts ${inCopy}px into it, the pane's ${inPane}px`]
            : []),
      ],
    }
  },
})

const meta = {
  name: "drag",
  summary:
    "pane drag: the copy's centre on the pointer, previews in the grid, zones that settle, every cancel",
  defaults: { engine: "chromium,webkit", layout: "columns,sidebar" },
  options: {
    only: { type: "string" },
    sizes: { type: "string", default: "1440x900,1000x700" },
    "reduced-motion": { type: "boolean", default: false },
    shots: { type: "string" },
  },
  help: `
Usage: node verification/desktop/scripts/drag.mjs [options]

  --only <list>    Checks, comma-separated. Available:
                   ${Object.keys(checks).join(", ")}
  --sizes <list>   Window sizes (default 1440x900,1000x700).
  --reduced-motion Every check with the system's reduced motion on.
  --shots <dir>    Screenshots from copy-takes-slot-shape go there.

sweep-across-zones covers follows-pointer, inside-grid, inside-window, one-way
and no-selection in one recorded drag. Side columns are hidden first so the
panes have the room. A check made for one layout runs only there.
boundary-jitter reads the model's edgeReach from the dev server; under
--mode prod that step is not run.`,
}

await main(meta, async ({ options, rep, url, mode }) => {
  const only = options.only
    ? chosen(options.only, Object.keys(checks), options.list)
    : null
  const sizes = options.choices("sizes").map((size) => {
    const [width, height] = size.split("x").map(Number)
    if (!width || !height) throw new CannotRun(`--sizes: ${size} is not WIDTHxHEIGHT`)
    return { width, height }
  })
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts)
      for (const { width, height } of sizes)
        for (const [name, check] of Object.entries(checks)) {
          if (only && !only.includes(name)) continue
          if (
            recordIfLeftOut(rep, mode, name, devServerOnlySteps.drag, {
              engine,
              layout,
              width: `${width}x${height}`,
            })
          )
            continue
          const run = typeof check === "function" ? check : check.run
          if (check.layouts && !check.layouts.includes(layout)) continue
          if (check.sizes && !check.sizes.includes(`${width}x${height}`)) continue
          await attempt(
            rep,
            { name, engine, layout, width: `${width}x${height}` },
            async () => {
              const pages = []
              const fresh = async (overrides = {}) => {
                const opened = await openPage(browser, {
                  url,
                  layout,
                  width,
                  height,
                  initScripts: [safeAreaInit],
                  reducedMotion: options["reduced-motion"] ? "reduce" : undefined,
                  ...overrides,
                })
                pages.push(opened)
                await need(opened.page, css.paneHeader, "a pane header")
                return opened.page
              }
              try {
                const result = await run(await fresh(), layout, fresh, {
                  shots: options.shots,
                  tag: `${engine}-${layout}-${width}x${height}`,
                })
                return {
                  ...result,
                  failures: [
                    ...(result.failures ?? []),
                    ...pages.flatMap((o) => o.errors),
                  ],
                }
              } finally {
                for (const opened of pages) await opened.close()
              }
            },
          )
        }
  })
})
