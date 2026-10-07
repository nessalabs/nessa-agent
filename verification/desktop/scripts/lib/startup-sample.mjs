/**
 * Turns one desktop load's browser clocks into the alpha sample.
 *
 * Navigation Timing and paint entries stay on the document's time origin.
 * CDP `Performance.getMetrics` names are Chromium's; a missing name is
 * absent, not zero. `performance.memory` fills the heap only when the
 * browser exposed it and CDP did not. Frame gaps are the rAF gaps
 * `observers` recorded (`lib/perf.mjs`). Nothing here decides a budget.
 */
import { median } from "./perf.mjs"

/** Gauges: the value now. Counters: work since the sample's own baseline. */
export const cdpGauges = [
  "JSHeapUsedSize",
  "JSHeapTotalSize",
  "Nodes",
  "Documents",
  "Frames",
  "JSEventListeners",
]
export const cdpCounters = [
  "LayoutCount",
  "RecalcStyleCount",
  "LayoutDuration",
  "RecalcStyleDuration",
  "ScriptDuration",
  "TaskDuration",
]
/** CDP metric names this sample keeps. Anything else is left out. */
export const cdpMetricNames = [...cdpGauges, ...cdpCounters]

/**
 * Durations on the navigation entry's clock. A missing entry is null.
 * `responseEnd` and the load marks are timestamps, so each duration is that
 * timestamp minus `startTime`. A zero `loadEventEnd` means the load event
 * has not fired, and the duration is null rather than negative.
 */
export function navigationDurations(entry) {
  if (!entry || typeof entry.startTime !== "number") return null
  const span = (end) =>
    typeof end === "number" && end > 0 ? end - entry.startTime : null
  return {
    durationMs: typeof entry.duration === "number" ? entry.duration : null,
    responseEndMs: span(entry.responseEnd),
    domContentLoadedMs: span(entry.domContentLoadedEventEnd),
    loadEventMs: span(entry.loadEventEnd),
    transferBytes: typeof entry.transferSize === "number" ? entry.transferSize : null,
    decodedBytes:
      typeof entry.decodedBodySize === "number" ? entry.decodedBodySize : null,
    type: typeof entry.type === "string" ? entry.type : null,
  }
}

/** `first-paint` and `first-contentful-paint` start times, or null when absent. */
export function paintStarts(entries) {
  const paints = { firstPaintMs: null, firstContentfulPaintMs: null }
  if (!Array.isArray(entries)) return paints
  for (const entry of entries) {
    if (!entry || typeof entry.startTime !== "number") continue
    if (entry.name === "first-paint") paints.firstPaintMs = entry.startTime
    if (entry.name === "first-contentful-paint")
      paints.firstContentfulPaintMs = entry.startTime
  }
  return paints
}

/** The allowlisted CDP metrics, keyed by name. Unknown names are dropped. */
export function cdpMetrics(list) {
  const table = {}
  if (!Array.isArray(list)) return table
  for (const item of list) {
    if (!item || typeof item.name !== "string") continue
    if (!cdpMetricNames.includes(item.name)) continue
    if (typeof item.value !== "number") continue
    table[item.name] = item.value
  }
  return table
}

/**
 * Gauges stay at their later reading. Counters become the difference, so a
 * warm sample does not include the cold load or the cache-fill reload.
 * A counter missing from the baseline counts from zero. A counter that
 * moved backwards is omitted: these CDP counters are not strictly
 * monotonic across a reload, and a negative duration is not a sample.
 */
export function metricsSince(before, after) {
  const start = cdpMetrics(before)
  const end = cdpMetrics(after)
  const table = {}
  for (const name of cdpGauges) {
    if (Object.hasOwn(end, name)) table[name] = end[name]
  }
  for (const name of cdpCounters) {
    if (!Object.hasOwn(end, name)) continue
    const from = Object.hasOwn(start, name) ? start[name] : 0
    if (end[name] < from) continue
    table[name] = end[name] - from
  }
  return table
}

/** One owned metric, or null when the table does not have that name. */
export function metric(table, name) {
  if (!table || !Object.hasOwn(table, name)) return null
  const value = table[name]
  return typeof value === "number" ? value : null
}

/**
 * Frames per second from the rAF gaps `[end, duration]` the caller kept.
 * The span is the sum of those durations. An empty list is null.
 */
export function fpsFromGaps(gaps) {
  if (!Array.isArray(gaps) || gaps.length === 0) return null
  const durations = gaps
    .map((gap) => (Array.isArray(gap) ? gap[1] : gap))
    .filter((duration) => typeof duration === "number" && duration > 0)
  if (durations.length === 0) return null
  const spanMs = durations.reduce((sum, duration) => sum + duration, 0)
  return {
    fps: (durations.length * 1000) / spanMs,
    frames: durations.length,
    spanMs,
    maxGapMs: Math.max(...durations),
  }
}

/** Count and longest duration of long tasks or Long Animation Frames. */
export function taskSummary(entries) {
  if (!Array.isArray(entries) || entries.length === 0) return { count: 0, maxMs: null }
  const durations = entries
    .map((entry) => entry?.duration)
    .filter((duration) => typeof duration === "number")
  return {
    count: entries.length,
    maxMs: durations.length ? Math.max(...durations) : null,
  }
}

/**
 * Milliseconds from the navigation's `startTime` to the first host endpoint
 * ask. The ask is `performance.now()` (`gatewayHost`). No ask, or no
 * navigation, is null.
 */
export function endpointAskMs(navigation, asks) {
  if (!navigation || typeof navigation.startTime !== "number") return null
  if (!Array.isArray(asks) || typeof asks[0] !== "number") return null
  return asks[0] - navigation.startTime
}

/**
 * One load. `readyMs` is the harness clock from just before navigation to
 * the settled page. Heap prefers CDP `JSHeapUsedSize`, then
 * `performance.memory` when that exists.
 */
export function summarizeStartup(raw) {
  const metrics = raw.metricTable ?? cdpMetrics(raw.metrics)
  const memory = raw.memory
  const heapFromMemory =
    memory && typeof memory.usedJSHeapSize === "number" ? memory.usedJSHeapSize : null
  const totalFromMemory =
    memory && typeof memory.totalJSHeapSize === "number" ? memory.totalJSHeapSize : null
  return {
    readyMs: typeof raw.readyMs === "number" ? raw.readyMs : null,
    cache: raw.cache ?? null,
    navigation: navigationDurations(raw.navigation),
    paints: paintStarts(raw.paints),
    heapUsed: metric(metrics, "JSHeapUsedSize") ?? heapFromMemory,
    heapTotal: metric(metrics, "JSHeapTotalSize") ?? totalFromMemory,
    nodes: metric(metrics, "Nodes"),
    documents: metric(metrics, "Documents"),
    layoutCount: metric(metrics, "LayoutCount"),
    recalcStyleCount: metric(metrics, "RecalcStyleCount"),
    scriptDuration: metric(metrics, "ScriptDuration"),
    layoutDuration: metric(metrics, "LayoutDuration"),
    taskDuration: metric(metrics, "TaskDuration"),
    dom: typeof raw.dom === "number" ? raw.dom : null,
    frames: fpsFromGaps(raw.gaps),
    longTasks: taskSummary(raw.longtasks),
    loaf: taskSummary(raw.loaf),
    endpointAskMs: endpointAskMs(raw.navigation, raw.endpointAsks),
  }
}

/**
 * Median and max of the numbers `read` returns, rounded for a table.
 * The JSON samples keep the unrounded values. An empty series is nulls.
 */
export function series(samples, read) {
  const values = []
  for (const sample of samples) {
    const value = read(sample)
    if (typeof value === "number" && Number.isFinite(value)) values.push(value)
  }
  if (values.length === 0) return { median: null, max: null, runs: "" }
  return {
    median: median(values.map((value) => Math.round(value))),
    max: Math.round(Math.max(...values)),
    runs: values.map((value) => Math.round(value)).join(" "),
  }
}
