/**
 * Frame timing for the performance budget (ADR 238, Context: "Calm means no
 * dropped frames" — no frame over 50 ms in a production build at 4× CPU
 * throttling).
 *
 * A frame is the gap between consecutive requestAnimationFrame callbacks.
 * Long Animation Frame entries (Chromium) and long tasks attribute a slow
 * frame to scripts and to forced style and layout.
 */
import { CannotRun } from "./cli.mjs"

export const budgetMs = 50

/** Installed as an init script: rAF gaps, LoAF and long-task buffers. */
export function observers() {
  window.__perf = { gaps: [], loaf: [], longtasks: [] }
  try {
    new PerformanceObserver((list) => {
      for (const e of list.getEntries())
        window.__perf.loaf.push({
          start: e.startTime,
          duration: e.duration,
          blocking: e.blockingDuration,
          renderStart: e.renderStart,
          styleAndLayoutStart: e.styleAndLayoutStart,
          scripts: e.scripts.map((s) => ({
            duration: Math.round(s.duration),
            invoker: s.invoker,
            type: s.invokerType,
            fn: s.sourceFunctionName,
            source: `${(s.sourceURL || "").split("/").pop()}:${s.sourceCharPosition}`,
            forcedLayout: Math.round(s.forcedStyleAndLayoutDuration),
          })),
        })
    }).observe({ type: "long-animation-frame", buffered: false })
  } catch {
    window.__perf.noLoaf = true
  }
  try {
    new PerformanceObserver((list) => {
      for (const e of list.getEntries())
        window.__perf.longtasks.push({ start: e.startTime, duration: e.duration })
    }).observe({ type: "longtask", buffered: false })
  } catch {
    window.__perf.noLongtask = true
  }
  let last = 0
  const tick = (t) => {
    if (last) window.__perf.gaps.push([t, t - last])
    last = t
    requestAnimationFrame(tick)
  }
  requestAnimationFrame(tick)
}

/** Sets CDP CPU throttling (Chromium only). */
export async function throttle(context, page, rate) {
  const cdp = await context.newCDPSession(page)
  await cdp.send("Emulation.setCPUThrottlingRate", { rate })
  return cdp
}

/**
 * The sanity check behind every number: a fixed busy loop, unthrottled and
 * throttled. If the ratio is far below the rate asked for, the throttle did
 * not apply and the table would flatter the page.
 */
export async function calibrate(context, page, rate) {
  const work = () =>
    page.evaluate(() => {
      const t = performance.now()
      let x = 0
      for (let i = 0; i < 3e7; i++) x += Math.sqrt(i)
      return performance.now() - t + (x < 0 ? 1 : 0)
    })
  await work() // warm the JIT
  const plain = await work()
  const cdp = await throttle(context, page, rate)
  const throttled = await work()
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 })
  const ratio = throttled / plain
  const expected = Math.max(1, rate * 0.6)
  return {
    rate,
    plainMs: Math.round(plain),
    throttledMs: Math.round(throttled),
    ratio: Number(ratio.toFixed(2)),
    ok: rate <= 1 || ratio >= expected,
  }
}

/**
 * A calibration frame of known cost: one animation frame that busy-waits
 * `cost` ms. The measurement is working only if it reports a frame at least
 * that long and a Long Animation Frame covering it. Needs `observers`.
 */
export async function calibrationFrame(page, cost = 120) {
  const m = await measure(
    page,
    () =>
      page.evaluate((cost) => {
        requestAnimationFrame(() => {
          const t = performance.now()
          while (performance.now() - t < cost) {
            // spend the frame
          }
        })
      }, cost),
    600,
  )
  const attributed = m.slow.some((s) => s.loaf && s.loaf.duration >= cost * 0.9)
  return {
    cost,
    measuredMs: m.maxFrame,
    attributed,
    ok: m.maxFrame >= cost * 0.9 && (attributed || m.noLoaf),
  }
}

/** Measures one interaction: frames from just before `act` to `settle` ms after. */
export async function measure(page, act, settle = 1000) {
  const t0 = await page.evaluate(() => performance.now())
  await act()
  await page.waitForTimeout(settle)
  const m = await page.evaluate((t0) => {
    const p = window.__perf
    return {
      gaps: p.gaps.filter(([t]) => t >= t0).map(([t, d]) => [t - t0, d]),
      loaf: p.loaf
        .filter((e) => e.start >= t0 - 5)
        .map((e) => ({ ...e, start: e.start - t0 })),
      longtasks: p.longtasks
        .filter((e) => e.start >= t0 - 5)
        .map((e) => ({ ...e, start: e.start - t0 })),
      noLoaf: !!p.noLoaf,
    }
  }, t0)
  if (m.gaps.length === 0)
    throw new CannotRun("no animation frames were recorded (is the page visible?)")
  const durations = m.gaps.map(([, d]) => d)
  const max = Math.max(...durations)
  return {
    maxFrame: Math.round(max),
    over: durations.filter((d) => d > budgetMs).length,
    frames: durations.length,
    slow: attribute(m),
    noLoaf: m.noLoaf,
  }
}

/**
 * For each over-budget frame, the Long Animation Frame that covers it: its
 * blocking time, how long style and layout took, and its longest scripts
 * with their forced layout.
 */
function attribute(m) {
  return m.gaps
    .filter(([, d]) => d > budgetMs)
    .map(([end, d]) => {
      const start = end - d
      const loaf = m.loaf.find((e) => e.start < end && e.start + e.duration > start)
      const task = m.longtasks.find((e) => e.start < end && e.start + e.duration > start)
      return {
        at: Math.round(start),
        frame: Math.round(d),
        longTask: task ? Math.round(task.duration) : null,
        loaf: loaf
          ? {
              duration: Math.round(loaf.duration),
              blocking: Math.round(loaf.blocking),
              styleAndLayout: loaf.styleAndLayoutStart
                ? Math.round(loaf.start + loaf.duration - loaf.styleAndLayoutStart)
                : null,
              scripts: loaf.scripts
                .slice()
                .sort((a, b) => b.duration - a.duration)
                .slice(0, 4),
            }
          : null,
      }
    })
}

export const median = (values) => {
  const sorted = values.slice().sort((a, b) => a - b)
  return sorted.length ? sorted[Math.floor(sorted.length / 2)] : null
}
