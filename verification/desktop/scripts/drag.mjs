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
 *                        window's controls, any frame
 *   docked-columns-no-zone  (1440 × 900) a pane carried over the docked sidebar, and over
 *                        the docked session list, offers no zone and no placeholder; the
 *                        release there changes nothing
 *   peek-press-while-hiding  the sidebar revealed from the edge, the pointer leaves it for a
 *                        pane's header and presses within the reveal's 350 ms hide: the
 *                        reveal stays, every frame of the drag; over it is no zone, just past
 *                        it a pane is a target; released away, it hides
 *   reduced-motion       with less motion, nothing but the copy moves: no pane is drawn
 *                        off its place while a zone is shown, and the placeholder marks it
 *
 * `--reduced-motion` runs every check with the system's reduced motion on.
 */
import { attempt, CannotRun, chosen } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { safeArea, safeAreaInit, summarize } from "./lib/safe-area.mjs"
import { content, css, keys, modules, zoneSaid } from "./lib/selectors.mjs"
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
    const still = await fresh({ reducedMotion: "reduce" })
    const list = await threeColumns(still, layout)
    const target = list[2]
    await still.evaluate((sel) => {
      window.__moved = new Set()
      window.__watch = true
      const tick = () => {
        document.querySelectorAll(sel.pane).forEach((p) => {
          if (p.closest(sel.dragGhost)) return
          const t = getComputedStyle(p).transform
          if (t !== "none" && !new DOMMatrix(t).isIdentity)
            window.__moved.add(p.dataset.paneKey)
        })
        if (window.__watch) requestAnimationFrame(tick)
      }
      requestAnimationFrame(tick)
    }, css)
    await lift(still, 0)
    await still.mouse.move(target.x + target.w - 20, target.y + target.h / 2, {
      steps: 20,
    })
    const said = await zoneSays(still, zoneSaid.any)
    await still.waitForTimeout(300)
    const placeholders = await still.locator(css.dragPlaceholder).count()
    const moved = await still.evaluate(() => {
      window.__watch = false
      return [...window.__moved]
    })
    await letGo(still, { escape: true })
    const failures = []
    if (!said) throw new CannotRun("no zone was offered over the far pane")
    if (moved.length)
      failures.push(`panes drawn off their place with less motion: ${moved.join(", ")}`)
    if (!placeholders) failures.push("no placeholder marks where the drop would land")
    failures.push(...residueFailures(await dragResidue(still)))
    return { failures }
  },
  "copy-under-controls": async (page, layout) => {
    await hideColumns(page, layout)
    await openPanes(page, 2)
    const sampler = safeArea(page)
    await sampler.watch(
      "carry the corner pane",
      async () => {
        await lift(page, 0)
        // Held near where it was grabbed while the copy glides to its centre.
        await page.waitForTimeout(300)
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
  },
  help: `
Usage: node verification/desktop/scripts/drag.mjs [options]

  --only <list>    Checks, comma-separated. Available:
                   ${Object.keys(checks).join(", ")}
  --sizes <list>   Window sizes (default 1440x900,1000x700).
  --reduced-motion Every check with the system's reduced motion on.

sweep-across-zones covers follows-pointer, inside-grid, inside-window, one-way
and no-selection in one recorded drag. Side columns are hidden first so the
panes have the room. A check made for one layout runs only there.`,
}

await main(meta, async ({ options, rep, url }) => {
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
                const result = await run(await fresh(), layout, fresh)
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
