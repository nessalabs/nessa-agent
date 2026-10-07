/**
 * The alpha startup sample reads durations off the navigation clock and
 * keeps only the CDP metric names it owns. A missing load event is absent,
 * not a negative duration. Heap falls back to `performance.memory` only
 * when CDP did not report it.
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import {
  cdpMetrics,
  endpointAskMs,
  fpsFromGaps,
  metric,
  metricsSince,
  navigationDurations,
  paintStarts,
  series,
  summarizeStartup,
  taskSummary,
} from "./startup-sample.mjs"

const navigation = {
  startTime: 10,
  duration: 240,
  responseEnd: 80,
  domContentLoadedEventEnd: 140,
  loadEventEnd: 250,
  transferSize: 1200,
  decodedBodySize: 4000,
  type: "navigate",
}

describe("startup sample", () => {
  it("subtracts the navigation start from the phase timestamps", () => {
    const durations = navigationDurations(navigation)
    assert.equal(durations.durationMs, 240)
    assert.equal(durations.responseEndMs, 70)
    assert.equal(durations.domContentLoadedMs, 130)
    assert.equal(durations.loadEventMs, 240)
    assert.equal(durations.transferBytes, 1200)
    assert.equal(durations.decodedBytes, 4000)
    assert.equal(durations.type, "navigate")
  })

  it("treats a zero load-event timestamp as the event not having fired", () => {
    const durations = navigationDurations({ ...navigation, loadEventEnd: 0 })
    assert.equal(durations.loadEventMs, null)
    assert.equal(durations.responseEndMs, 70)
  })

  it("returns null when the document has no navigation entry", () => {
    assert.equal(navigationDurations(null), null)
    assert.equal(navigationDurations({ duration: 10 }), null)
  })

  it("keeps the two paint names and ignores the rest", () => {
    const paints = paintStarts([
      { name: "first-paint", startTime: 40 },
      { name: "first-contentful-paint", startTime: 55.5 },
      { name: "other", startTime: 9 },
    ])
    assert.deepEqual(paints, { firstPaintMs: 40, firstContentfulPaintMs: 55.5 })
    assert.deepEqual(paintStarts(null), {
      firstPaintMs: null,
      firstContentfulPaintMs: null,
    })
  })

  it("keeps allowlisted CDP metrics and drops names it does not own", () => {
    const table = cdpMetrics([
      { name: "JSHeapUsedSize", value: 1000 },
      { name: "Nodes", value: 42 },
      { name: "ScriptDuration", value: 12.5 },
      { name: "UnknownMetric", value: 99 },
      { name: "TaskDuration", value: "slow" },
    ])
    assert.equal(metric(table, "JSHeapUsedSize"), 1000)
    assert.equal(metric(table, "Nodes"), 42)
    assert.equal(metric(table, "ScriptDuration"), 12.5)
    assert.equal(metric(table, "UnknownMetric"), null)
    assert.equal(metric(table, "TaskDuration"), null)
    assert.equal(Object.hasOwn(table, "UnknownMetric"), false)
  })

  it("does not read a CDP name inherited from Object.prototype", () => {
    Object.defineProperty(Object.prototype, "LayoutCount", {
      configurable: true,
      get() {
        throw new Error("inherited LayoutCount")
      },
    })
    try {
      const table = metricsSince(
        [{ name: "JSHeapUsedSize", value: 1 }],
        [
          { name: "JSHeapUsedSize", value: 2 },
          { name: "Nodes", value: 3 },
        ],
      )
      assert.equal(Object.hasOwn(table, "LayoutCount"), false)
      assert.equal(metric(table, "LayoutCount"), null)
      assert.equal(metric({}, "LayoutCount"), null)
      assert.equal(table.JSHeapUsedSize, 2)
      assert.equal(table.Nodes, 3)
    } finally {
      delete Object.prototype.LayoutCount
    }
  })

  it("keeps gauges and subtracts counters from the sample baseline", () => {
    const table = metricsSince(
      [
        { name: "LayoutCount", value: 10 },
        { name: "JSHeapUsedSize", value: 100 },
        { name: "ScriptDuration", value: 4 },
      ],
      [
        { name: "LayoutCount", value: 14 },
        { name: "JSHeapUsedSize", value: 250 },
        { name: "ScriptDuration", value: 9 },
        { name: "Nodes", value: 3 },
        { name: "TaskDuration", value: 8 },
      ],
    )
    assert.equal(table.LayoutCount, 4)
    assert.equal(table.ScriptDuration, 5)
    assert.equal(table.JSHeapUsedSize, 250)
    assert.equal(table.Nodes, 3)
    assert.equal(table.TaskDuration, 8)
    assert.equal(metric(table, "LayoutCount"), 4)
  })

  it("omits a counter that moved backwards", () => {
    const table = metricsSince(
      [
        { name: "LayoutCount", value: 10 },
        { name: "ScriptDuration", value: 4 },
      ],
      [
        { name: "LayoutCount", value: 9 },
        { name: "ScriptDuration", value: 4.5 },
        { name: "JSHeapUsedSize", value: 8 },
      ],
    )
    assert.equal(Object.hasOwn(table, "LayoutCount"), false)
    assert.equal(metric(table, "LayoutCount"), null)
    assert.equal(table.ScriptDuration, 0.5)
    assert.equal(table.JSHeapUsedSize, 8)
  })

  it("computes frames per second from the rAF gaps", () => {
    const frames = fpsFromGaps([
      [16.6, 16.6],
      [33.3, 16.7],
    ])
    assert.equal(frames.frames, 2)
    assert.equal(frames.spanMs, 33.3)
    assert.equal(frames.maxGapMs, 16.7)
    assert.ok(Math.abs(frames.fps - 2000 / 33.3) < 1e-9)
    assert.equal(fpsFromGaps([]), null)
    assert.equal(fpsFromGaps(null), null)
  })

  it("summarizes long tasks by count and longest duration", () => {
    assert.deepEqual(taskSummary([{ duration: 12 }, { duration: 80.2 }]), {
      count: 2,
      maxMs: 80.2,
    })
    assert.deepEqual(taskSummary([]), { count: 0, maxMs: null })
  })

  it("times the first endpoint ask from the navigation start", () => {
    assert.equal(endpointAskMs(navigation, [90, 140]), 80)
    assert.equal(endpointAskMs(navigation, []), null)
    assert.equal(endpointAskMs(null, [90]), null)
  })

  it("prefers the CDP heap and falls back to performance.memory", () => {
    const withCdp = summarizeStartup({
      readyMs: 300,
      cache: "disabled",
      navigation,
      paints: [{ name: "first-contentful-paint", startTime: 55 }],
      metrics: [
        { name: "JSHeapUsedSize", value: 5000 },
        { name: "Nodes", value: 80 },
      ],
      memory: { usedJSHeapSize: 1, totalJSHeapSize: 2 },
      gaps: [[16, 16]],
      longtasks: [{ duration: 40 }],
      loaf: [],
      endpointAsks: [30],
      dom: 90,
    })
    assert.equal(withCdp.heapUsed, 5000)
    assert.equal(withCdp.nodes, 80)
    assert.equal(withCdp.paints.firstContentfulPaintMs, 55)
    assert.equal(withCdp.longTasks.maxMs, 40)
    assert.equal(withCdp.loaf.count, 0)
    assert.equal(withCdp.endpointAskMs, 20)
    assert.equal(withCdp.dom, 90)
    assert.equal(withCdp.frames.frames, 1)

    const memoryOnly = summarizeStartup({
      navigation,
      memory: { usedJSHeapSize: 7, totalJSHeapSize: 9 },
    })
    assert.equal(memoryOnly.heapUsed, 7)
    assert.equal(memoryOnly.heapTotal, 9)
    assert.equal(memoryOnly.navigation.durationMs, 240)
    assert.equal(memoryOnly.readyMs, null)
  })

  it("rounds a series for the table and leaves an empty series blank", () => {
    const samples = [{ readyMs: 100.4 }, { readyMs: 180.6 }, { readyMs: null }]
    const ready = series(samples, (sample) => sample.readyMs)
    assert.equal(ready.max, 181)
    assert.equal(ready.median, 181)
    assert.equal(ready.runs, "100 181")
    assert.deepEqual(
      series([], (sample) => sample.readyMs),
      {
        median: null,
        max: null,
        runs: "",
      },
    )
  })
})
