/**
 * Driving and reading the workspace: pane rects and order, opening panes
 * beside through the quick switcher (the same in both layouts), the side
 * columns, the drop zone the drag announces, whether one rect is inside
 * another, and a model module's rule or value read in the page.
 */
import { CannotRun } from "./cli.mjs"
import { css, keys, names, preferenceEvents, storage } from "./selectors.mjs"

/** Every pane's key, rect and focus, in document order. */
export function panes(page) {
  return page.evaluate(
    ([sel, ghost, focused]) => {
      return [...document.querySelectorAll(sel)]
        .filter((e) => !e.closest(ghost))
        .map((e) => {
          const r = e.getBoundingClientRect()
          return {
            key: e.dataset.paneKey,
            focused: e.matches(focused),
            x: r.left,
            y: r.top,
            w: r.width,
            h: r.height,
          }
        })
    },
    [css.pane, css.dragGhost, css.focusedPane],
  )
}

/** Pane keys ordered as they read: top to bottom within left-to-right columns. */
export async function order(page) {
  const list = await panes(page)
  return list
    .slice()
    .sort((a, b) => Math.round(a.x - b.x) || Math.round(a.y - b.y))
    .map((p) => p.key)
}

export const paneCount = async (page) => (await panes(page)).length

/** What fills the content region (see `content` in selectors.mjs) and where the caret is. */
export function state(page) {
  return page.evaluate(
    ([workspace, pane, focusedPane, overviewItem]) => {
      const a = document.activeElement
      return {
        content: document.querySelector(workspace)?.dataset.content ?? null,
        active: a
          ? `${a.tagName}${a.getAttribute("aria-label") ? `[${a.getAttribute("aria-label")}]` : ""}`
          : null,
        activeInPane: a?.closest(pane)?.dataset.paneKey ?? null,
        activeIsComposer: a?.tagName === "TEXTAREA" && !!a.closest(pane),
        activeOverviewItem: a?.closest(overviewItem)?.dataset.overviewItem ?? null,
        focusedPane: document.querySelector(focusedPane)?.dataset.paneKey ?? null,
      }
    },
    [css.workspace, css.pane, css.focusedPane, css.overviewItem],
  )
}

/**
 * Waits until `predicate(arg)` holds in the page — at most `timeout` ms —
 * and says whether it did. What a step waits on is a condition, never a
 * fixed time.
 */
export async function until(page, predicate, arg, timeout = 3000) {
  try {
    await page.waitForFunction(predicate, arg, { timeout, polling: "raf" })
    return true
  } catch {
    return false
  }
}

/** Waits out `n` of the page's frames: what a step does lands a frame or two after it. */
export const frames = (page, n = 2) =>
  page.evaluate(
    (count) =>
      new Promise((done) => {
        const next = (left) =>
          left ? requestAnimationFrame(() => next(left - 1)) : done()
        next(count)
      }),
    n,
  )

/**
 * Waits until this open has drawn every overview row (`data-overview-listed`).
 * A count taken before that is the prefix, not the list.
 */
export const overviewListed = (page, timeout = 2000) =>
  until(page, (sel) => document.querySelector(sel) !== null, css.overviewListed, timeout)

/** How many requests the Agents overview lists (it must be open). */
export const requestCount = (page) => page.locator(css.overviewRequest).count()

/** Waits until the page has `n` panes; says whether it did. */
export const paneCountIs = (page, n, timeout) =>
  until(
    page,
    ([pane, ghost, count]) =>
      [...document.querySelectorAll(pane)].filter((e) => !e.closest(ghost)).length ===
      count,
    [css.pane, css.dragGhost, n],
    timeout,
  )

/** Waits until the content region shows `value` (see `content`); says whether it did. */
export const contentIs = (page, value, timeout) =>
  until(
    page,
    ([workspace, want]) => document.querySelector(workspace)?.dataset.content === want,
    [css.workspace, value],
    timeout,
  )

/**
 * Opens panes beside until there are `n`, picking sessions in the quick
 * switcher (⌘\\, then the i-th row). Fails as could-not-run if the count
 * does not grow — the UI is not what the script expects.
 */
export async function openPanes(page, n) {
  for (let i = 1; (await paneCount(page)) < n; i++) {
    const before = await paneCount(page)
    if (i > n + 3) throw new CannotRun(`could not open ${n} panes (stuck at ${before})`)
    await page.keyboard.press(keys.openBeside)
    await page.waitForSelector(css.switcherField, { state: "visible", timeout: 3000 })
    for (let j = 0; j <= i; j++) await page.keyboard.press(keys.down)
    await page.keyboard.press(keys.enter)
    await paneCountIs(page, before + 1)
    await settled(page)
  }
}

/** Puts the caret in the focused pane's composer, where the window's keys are pressed from. */
export async function focusComposer(page) {
  const composer = page.locator(`${css.focusedPane} ${css.field}`).first()
  if (!(await composer.count()))
    throw new CannotRun(
      `no composer in the focused pane (${css.focusedPane} ${css.field})`,
    )
  await composer.click()
  await until(
    page,
    ([focused, field]) => document.activeElement?.matches(`${focused} ${field}`) === true,
    [css.focusedPane, css.field],
  )
}

/** Leaves Settings the way a person does, by its Back button. */
export async function leaveSettings(page) {
  const back = page.getByRole("button", { name: names.leaveSettings }).first()
  if (!(await back.count()))
    throw new CannotRun(`no "${names.leaveSettings}" button in Settings`)
  await back.click()
}

/** Hides the side columns so the panes have the room (⌘B, and ⌥⌘S in three columns). */
export async function hideColumns(page, layout) {
  await page.keyboard.press(keys.toggleSidebar)
  if (layout === "columns") await page.keyboard.press(keys.toggleSessionList)
  await settled(page)
}

/** Changes the layout the way Settings does: the stored preference and its event. */
export function switchLayout(page, layout) {
  return page.evaluate(
    ([key, event, value]) => {
      localStorage.setItem(key, value)
      window.dispatchEvent(new CustomEvent(event, { detail: value }))
    },
    [storage.layout, preferenceEvents.layout, layout],
  )
}

/** Records announced drop zones; one-shot `take()` stops recording and releases its resources. */
export async function recordZones(page) {
  const recorder = await page.evaluateHandle((sel) => {
    const status = document.querySelector(sel)
    if (!status) return null
    const records = []
    let lastMoveAt = performance.now()
    let position
    const moved = (event) => {
      if (position?.x === event.clientX && position?.y === event.clientY) return
      position = { x: event.clientX, y: event.clientY }
      lastMoveAt = event.timeStamp
    }
    document.addEventListener("pointermove", moved, true)
    const observer = new MutationObserver(() => {
      if (status.textContent)
        records.push({ said: status.textContent, at: performance.now(), lastMoveAt })
    })
    observer.observe(status, { childList: true, characterData: true, subtree: true })
    return {
      take() {
        observer.disconnect()
        document.removeEventListener("pointermove", moved, true)
        return records.splice(0)
      },
    }
  }, css.dropAnnouncer)
  if (!(await recorder.evaluate((value) => value !== null))) {
    await recorder.dispose()
    throw new CannotRun(`no drop announcer (${css.dropAnnouncer})`)
  }
  return {
    take: async () => {
      try {
        return await recorder.evaluate((value) => value.take())
      } finally {
        await recorder.dispose()
      }
    },
  }
}

/**
 * Presses a pane by its title — never a titlebar control over its header —
 * and moves past the lift distance, leaving the pointer down. Returns where
 * the pointer is.
 */
export async function lift(page, index = 0) {
  const at = await page.evaluate(
    ([title, handle, button, i]) => {
      const element = document.querySelectorAll(title)[i]
      if (!element) return null
      const r = element.getBoundingClientRect()
      // Left of the title's middle: a long title runs under nothing but itself.
      const x = r.left + Math.min(r.width / 2, 40)
      const y = r.top + r.height / 2
      const hit = document.elementFromPoint(x, y)
      return {
        x,
        y,
        ok: !!hit && !!hit.closest(handle) && !hit.closest(button),
        hit: hit ? `${hit.tagName}.${String(hit.className).slice(0, 40)}` : "nothing",
      }
    },
    [css.paneTitle, css.paneDragHandle, css.button, index],
  )
  if (!at) throw new CannotRun(`no pane title ${index} (${css.paneTitle})`)
  if (!at.ok)
    throw new CannotRun(
      `pane ${index}'s title is under ${at.hit} at ${Math.round(at.x)},${Math.round(at.y)}, not its header`,
    )
  await page.mouse.move(at.x, at.y)
  await page.mouse.down()
  // A press becomes a drag once its copy is made, after the press's frame
  // has painted (ADR 238, "Drag and drop"): a person's hand is that slow.
  await page.waitForSelector(css.dragGhost, { state: "attached", timeout: 2000 })
  for (let i = 1; i <= 6; i++) await page.mouse.move(at.x + i * 5, at.y + i * 5)
  try {
    await page.waitForSelector(`${css.dragGhost}:not([data-drag-waiting])`, {
      state: "visible",
      timeout: 2000,
    })
  } catch {
    throw new CannotRun(
      `pressing and moving pane ${index}'s title did not lift a copy (${css.dragGhost})`,
    )
  }
  return { x: at.x + 30, y: at.y + 30 }
}

/** Waits until nothing finite is animating on the page — at most `timeout` ms. */
export async function settled(page, timeout = 3000) {
  await page.waitForFunction(
    () =>
      !document
        .getAnimations()
        .some(
          (a) =>
            (a.playState === "running" || a.playState === "pending") &&
            a.effect?.getTiming?.().iterations !== Infinity,
        ),
    null,
    { timeout, polling: "raf" },
  )
}

/** Waits for the drop announcer to say something matching `pattern` (a RegExp, or "" for nothing). */
export async function zoneSays(page, pattern, timeout = 2000) {
  const source = pattern instanceof RegExp ? pattern.source : null
  try {
    await page.waitForFunction(
      ([sel, src]) => {
        const text = document.querySelector(sel)?.textContent ?? ""
        return src === null ? text === "" : new RegExp(src).test(text)
      },
      [css.dropAnnouncer, source],
      { timeout, polling: "raf" },
    )
    return true
  } catch {
    return false
  }
}

/** What a drag left on the page: copies, placeholders, marks, and panes drawn off their place. */
export function dragResidue(page) {
  return page.evaluate(
    (sel) => ({
      copies: document.querySelectorAll(sel.dragCarrier).length,
      placeholders: document.querySelectorAll(sel.dragPlaceholder).length,
      shields: document.querySelectorAll(sel.dragShield).length,
      lifted: document.querySelectorAll(sel.lifted).length,
      dragging: document.querySelectorAll(sel.dragging).length,
      transformed: [...document.querySelectorAll(sel.pane)]
        .filter((p) => !p.closest(sel.dragGhost))
        .filter((p) => {
          const transform = getComputedStyle(p).transform
          // A finished flight holds the identity: in place.
          return transform !== "none" && !new DOMMatrix(transform).isIdentity
        })
        .map((p) => p.dataset.paneKey),
      // Laid out at a size of the drag's preview rather than the stylesheet's.
      sized: [...document.querySelectorAll(sel.pane)]
        .filter((p) => !p.closest(sel.dragGhost))
        .filter((p) => p.style.width || p.style.height)
        .map((p) => p.dataset.paneKey),
    }),
    css,
  )
}

/** Failure lines for anything a finished drag left behind. */
export function residueFailures(residue) {
  const out = []
  for (const key of ["copies", "placeholders", "shields", "lifted", "dragging"])
    if (residue[key]) out.push(`${residue[key]} ${key} left after the drag`)
  if (residue.transformed.length)
    out.push(`panes left drawn off their place: ${residue.transformed.join(",")}`)
  if (residue.sized.length)
    out.push(`panes left at a size of the drag's: ${residue.sized.join(",")}`)
  return out
}

/**
 * Records, every frame: the pointer, the copy's rect, the grid, the viewport,
 * each pane's rect and the zone said, and any selected text. `stop()`
 * returns the frames.
 */
export async function recordFrames(page) {
  await page.evaluate((sel) => {
    window.__dragFrames = []
    window.__dragOn = true
    const rect = (e) => {
      const r = e.getBoundingClientRect()
      return { x: r.left, y: r.top, w: r.width, h: r.height }
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
      const grid = document.querySelector(sel.paneGrid)
      window.__dragFrames.push({
        t,
        pointer: window.__verifyPointer ?? null,
        ghost:
          ghost && ghost.checkVisibility({ checkOpacity: true }) ? rect(ghost) : null,
        grid: grid ? rect(grid) : null,
        viewport: { w: innerWidth, h: innerHeight },
        panes: [...document.querySelectorAll(sel.pane)]
          .filter((e) => !e.closest(sel.dragGhost))
          .map((e) => ({ key: e.dataset.paneKey, ...rect(e) })),
        zone: document.querySelector(sel.dropAnnouncer)?.textContent ?? "",
        selection: String(getSelection() ?? "")
          .trim()
          .slice(0, 40),
      })
      if (window.__dragOn) requestAnimationFrame(tick)
    }
    requestAnimationFrame(tick)
  }, css)
  return {
    stop: () =>
      page.evaluate(() => {
        window.__dragOn = false
        return window.__dragFrames
      }),
  }
}

/**
 * Whether rect `inner` lies inside rect `outer` on each of `sides` (all four
 * unless said), to half a pixel: subpixel layout is not a failure.
 */
export function inside(inner, outer, sides = ["left", "top", "right", "bottom"]) {
  const holds = {
    left: inner.left >= outer.left - 0.5,
    top: inner.top >= outer.top - 0.5,
    right: inner.right <= outer.right + 0.5,
    bottom: inner.bottom <= outer.bottom + 0.5,
  }
  return sides.every((side) => holds[side])
}

/**
 * Reads `name` from a model module in the page (`modules`): the owner's own
 * export, so a script never keeps a copy of it. With `args`, `name` is a
 * function and its answer is returned; without, the export itself (a plain
 * value). Needs the dev server, which serves the source; a production build
 * has no module to import, and that is "could not run".
 */
async function fromModel(page, module, name, args) {
  try {
    return await page.evaluate(
      async ([path, key, list]) => {
        const exported = (await import(path))[key]
        return list === null ? exported : exported(...list)
      },
      [module, name, args],
    )
  } catch (error) {
    throw new CannotRun(
      `could not read ${name} from ${module} in the page (it needs --mode dev): ${error.message.split("\n")[0]}`,
    )
  }
}

/** Calls a model rule in the page (`fromModel`). */
export const modelRule = (page, module, name, ...args) =>
  fromModel(page, module, name, args)

/** Reads a model value in the page (`fromModel`), such as a table of numbers. */
export const modelValue = (page, module, name) => fromModel(page, module, name, null)

/**
 * Failures when `root` draws an answer `options` does not offer (#444).
 * `options` are the review's own (`{ label }`). Each `button[data-answer]`
 * is named by its `aria-label`, or by its text when it has none. "More Ways
 * to Allow" is drawn only for an always answer, which a gateway review does
 * not offer, so it is a failure here too.
 */
export async function unofferedAnswers(root, options) {
  const labels = await root.locator("button[data-answer]").evaluateAll((elements) =>
    elements.map((element) => {
      const labelled = element.getAttribute("aria-label")
      if (labelled) return labelled
      return (element.textContent || "").replace(/\s+/g, " ").trim()
    }),
  )
  const offered = (options ?? []).map((option) => option.label)
  const failures = []
  for (const label of labels) {
    if (!offered.includes(label))
      failures.push(`offers "${label}", which this review does not`)
  }
  for (const label of offered) {
    if (!labels.includes(label))
      failures.push(`omits "${label}", which this review offers`)
  }
  if (
    (await root
      .getByRole("button", { name: names.moreWaysToAllow, exact: true })
      .count()) > 0
  )
    failures.push(`offers ${names.moreWaysToAllow}, which this review does not`)
  return failures
}

/** A measured pane drop must move it and remove the carried copy. */
export function paneDropFailure(before, after) {
  if (after.ghost !== 0) return "the carried copy is still on the page after the drop"
  if (after.order === before.order) return "the drop did not change pane order"
  return null
}
