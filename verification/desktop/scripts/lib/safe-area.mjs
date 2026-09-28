/**
 * The per-frame safe-area sampler (ADR 238, "The titlebar's safe area").
 *
 * Every animation frame for the watched window, it walks the page for
 * anything painted that reads, takes a press or pictures — text, images, SVG icons, canvas,
 * inputs, buttons — and records any whose painted box, clipped by its
 * clipping ancestors, enters the window controls' corner: x below
 * `--desktop-titlebar-safe-start`, y below `--desktop-titlebar-height`,
 * both resolved against the top-most surface. Titlebar rows, floating
 * layers and decorative art are exempt (`safeAreaExempt`).
 */
import { safeAreaExempt, safeAreaTokens, css } from "./selectors.mjs"

/**
 * Installed as an init script; defines `window.__safeWatch(name, cap)`, which
 * samples every frame, and `window.__safeSettled(least)`, which resolves once
 * the step has run `least` ms after its act and nothing finite is animating —
 * or at `cap` — so a step lasts as long as its motion, not a fixed wait.
 */
function sampler([exempt, tokens, surfaceSel]) {
  window.__safeBad = []
  window.__safeFrames = 0
  // The corner's size, read once per surface: a probe added and removed every
  // frame would make every frame lay the page out twice.
  const corners = new WeakMap()
  const cornerOf = (surface) => {
    const held = corners.get(surface)
    if (held) return held
    const px = (token) => {
      const probe = document.createElement("div")
      probe.style.cssText = `position:absolute;visibility:hidden;width:var(${token})`
      surface.append(probe)
      const width = probe.getBoundingClientRect().width
      probe.remove()
      return width
    }
    const corner = { safe: px(tokens.start), bar: px(tokens.height) }
    corners.set(surface, corner)
    return corner
  }
  // Still moving: a finite animation or transition running. A spinner's
  // endless turn is not motion a step waits for.
  const moving = () =>
    document
      .getAnimations()
      .some(
        (a) =>
          (a.playState === "running" || a.playState === "pending") &&
          a.effect?.getTiming?.().iterations !== Infinity,
      )
  let step = null
  window.__safeWatch = (name, cap = 800) => {
    const t0 = performance.now()
    let finish
    step = {
      name,
      t0,
      cap,
      actedAt: null,
      least: 0,
      done: new Promise((r) => (finish = r)),
    }
    const current = step
    const tick = () => {
      window.__safeFrames++
      const surface = [...document.querySelectorAll(surfaceSel)].at(-1)
      if (surface) {
        const { safe, bar } = cornerOf(surface)
        for (const e of document.body.querySelectorAll("*")) {
          // Where it is first, which is cheap on a laid-out page: almost
          // nothing is in the corner, and the rest is skipped before any style
          // or selector is asked of it.
          const r = e.getBoundingClientRect()
          if (r.right <= 0 || r.left >= safe || r.top >= bar || r.bottom <= 0) continue
          if (r.width === 0 && r.height === 0) continue
          const replaced = /^(IMG|CANVAS|VIDEO|svg|PRE|INPUT|TEXTAREA|BUTTON)$/.test(
            e.tagName,
          )
          const text = [...e.childNodes].some(
            (n) => n.nodeType === 3 && n.textContent.trim(),
          )
          if (!replaced && !text) continue
          if (e.parentElement?.closest("svg")) continue
          if (e.closest(exempt)) continue
          if (
            e.checkVisibility &&
            !e.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })
          )
            continue
          let box = { l: r.left, t: r.top, r: r.right, b: r.bottom }
          for (
            let a = e.parentElement;
            a && box.r > box.l && box.b > box.t;
            a = a.parentElement
          ) {
            const style = getComputedStyle(a)
            if (
              style.overflowX !== "visible" ||
              style.overflowY !== "visible" ||
              style.clipPath !== "none"
            ) {
              const ar = a.getBoundingClientRect()
              box = {
                l: Math.max(box.l, ar.left),
                t: Math.max(box.t, ar.top),
                r: Math.min(box.r, ar.right),
                b: Math.min(box.b, ar.bottom),
              }
            }
          }
          const inWidth = Math.min(box.r, safe) - Math.max(box.l, 0)
          const inHeight = Math.min(box.b, bar) - Math.max(box.t, 0)
          if (inWidth > 1 && inHeight > 1) {
            const label = (e.textContent || e.getAttribute("aria-label") || "")
              .trim()
              .slice(0, 24)
            const cls = String(e.className?.baseVal ?? e.className ?? "").slice(0, 40)
            window.__safeBad.push({
              step: name,
              at: Math.round(performance.now() - t0),
              element: `${e.tagName}.${cls}`,
              label,
              box: [
                Math.round(box.l),
                Math.round(box.t),
                Math.round(box.r),
                Math.round(box.b),
              ],
              safe: Math.round(safe),
              bar: Math.round(bar),
            })
          }
        }
      }
      const now = performance.now()
      const settled =
        current.actedAt !== null && now - current.actedAt >= current.least && !moving()
      if (step === current && !settled && now - t0 < cap) requestAnimationFrame(tick)
      else finish()
    }
    requestAnimationFrame(tick)
  }
  window.__safeSettled = (least) => {
    if (!step) return Promise.resolve()
    step.actedAt = performance.now()
    step.least = least
    return step.done
  }
}

/** The init script and its argument, for `openPage({ initScripts })`. */
export const safeAreaInit = [sampler, [safeAreaExempt, safeAreaTokens, css.surface]]

/**
 * Returns `watch(name, act, ms)`: samples every frame while `act` runs and
 * after, until the step has run at least `least` ms past its act and nothing
 * is animating — at most `ms` past its start — and `take()`: the violations
 * since the last take.
 */
export function safeArea(page, { least = 250 } = {}) {
  return {
    async watch(name, act, ms = 800) {
      await page.evaluate(([n, m]) => window.__safeWatch(n, m), [name, ms + 100])
      await act()
      await page.evaluate((l) => window.__safeSettled(l), least)
    },
    take: () => page.evaluate(() => window.__safeBad.splice(0)),
    frames: () => page.evaluate(() => window.__safeFrames),
  }
}

/** Groups violations by step and element, for a readable failure line. */
export function summarize(bad) {
  const groups = new Map()
  for (const b of bad) {
    const key = `${b.step}: ${b.element} "${b.label}" (safe ${b.safe}, bar ${b.bar})`
    const g = groups.get(key) ?? { count: 0, first: b.at, box: b.box }
    g.count++
    groups.set(key, g)
  }
  return [...groups].map(
    ([key, g]) =>
      `${key} — ${g.count} frame(s), first at ${g.first}ms [${g.box.join(",")}]`,
  )
}
