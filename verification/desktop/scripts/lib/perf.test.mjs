/**
 * The frame budget decides on the unrounded duration. Presentation rounding
 * is not an input: a 50.1 ms frame fails, and an exact 50 ms frame and a
 * 49.6 ms frame pass. The Long Animation Frame sample shifts every timestamp
 * off the performance clock together, so style and layout is not a negative
 * duration (#369).
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"
import { runInNewContext } from "node:vm"

import { CannotRun } from "./cli.mjs"
import {
  budgetMs,
  interactionBudget,
  median,
  measurementFrom,
  missingFrameSample,
  observers,
  sampleLoaf,
  styleAndLayoutMs,
} from "./perf.mjs"

const script = {
  duration: 38,
  invoker: "pointerup",
  type: "event-listener",
  fn: "onPointerUp",
  source: "pane.js:10",
  forcedLayout: 30,
}

it("samples callback execution rather than delayed nominal frame timestamps", () => {
  let now = 50
  const callbacks = []
  const scope = {
    window: {},
    performance: { now: () => now },
    requestAnimationFrame: (callback) => callbacks.push(callback),
    PerformanceObserver: class {
      observe() {}
    },
  }
  runInNewContext(`(${observers.toString()})()`, scope)
  callbacks.shift()(20)
  now = 200
  callbacks.shift()(120)
  assert.equal(scope.window.__perf.gaps.length, 1)
  assert.equal(scope.window.__perf.gaps[0][0], 200)
  assert.equal(scope.window.__perf.gaps[0][1], 150)
})

it("retains a first frame whose execution clock starts at zero", () => {
  let now = 0
  const callbacks = []
  const scope = {
    window: {},
    performance: { now: () => now },
    requestAnimationFrame: (callback) => callbacks.push(callback),
    PerformanceObserver: class {
      observe() {}
    },
  }
  runInNewContext(`(${observers.toString()})()`, scope)
  callbacks.shift()(0)
  now = 10
  callbacks.shift()(10)
  assert.equal(scope.window.__perf.gaps.length, 1)
  assert.equal(scope.window.__perf.gaps[0][1], 10)
})

it("keeps median precision and averages the middle pair of an even series", () => {
  const samples = [50.1, 33.2, 49.8, 50.3]
  assert.equal(median(samples), 49.95)
  assert.deepEqual(samples, [50.1, 33.2, 49.8, 50.3])
  assert.equal(median([]), null)
  const budget = interactionBudget(samples.map((maxFrame) => ({ maxFrame, over: 0 })))
  assert.equal(budget.median, 49.95)
  assert.equal(budget.maxFrame, 50.3)
  assert.equal(budget.exceeded, true)
})

const raw = (gaps, loaf, longtasks = []) => ({
  gaps,
  loaf,
  longtasks,
  noLoaf: false,
})

describe("frame budget", () => {
  it("fails a 50.1 ms frame whose presented maximum rounds to the budget", () => {
    const measured = measurementFrom(
      raw(
        [
          [10_033, 33],
          [10_083.1, 50.1],
          [10_133.1, 50],
        ],
        [],
      ),
      10_000,
    )
    assert.equal(measured.maxFrame, 50.1)
    assert.equal(measured.over, 1)
    assert.equal(measured.slow[0].frame, 50.1)

    const decision = interactionBudget([
      { maxFrame: 33, over: 0 },
      measured,
      { maxFrame: 50, over: 0 },
    ])
    assert.equal(decision.maxFrame, 50.1)
    assert.equal(decision.presentedMax, 50)
    assert.equal(decision.presentedRuns, "33 50 50")
    assert.equal(decision.over50, 1)
    // The rounded maximum is not over the budget. Comparing it, or
    // Math.round of the maximum, accepts this row and this assertion fails.
    assert.equal(decision.presentedMax > budgetMs, false)
    assert.equal(decision.exceeded, true)
  })

  it("accepts an exact 50 ms frame", () => {
    const measured = measurementFrom(raw([[10_050, 50]], []), 10_000)
    assert.equal(measured.maxFrame, 50)
    assert.equal(measured.over, 0)
    assert.equal(measured.slow.length, 0)
    const decision = interactionBudget([measured])
    assert.equal(decision.presentedMax, 50)
    assert.equal(decision.exceeded, false)
  })

  it("accepts a 49.6 ms frame that presentation rounds up to the budget", () => {
    const measured = measurementFrom(raw([[10_049.6, 49.6]], []), 10_000)
    assert.equal(measured.maxFrame, 49.6)
    assert.equal(measured.over, 0)
    const decision = interactionBudget([measured])
    assert.equal(decision.presentedMax, 50)
    assert.equal(decision.presentedMax > budgetMs, false)
    assert.equal(decision.exceeded, false)
  })

  it("could not run when the sample recorded no frame", () => {
    assert.throws(() => measurementFrom(raw([[10, 16]], []), 20), CannotRun)
  })

  it("keeps a missing frame sample as a gap and rethrows anything else", () => {
    const gap = new CannotRun("no animation frames were recorded (is the page visible?)")
    assert.equal(missingFrameSample(gap), gap.message)
    assert.throws(() => missingFrameSample(new Error("boom")), /boom/)
  })
})

describe("long animation frame clock", () => {
  it("shifts style and layout with the frame, so the duration stays positive", () => {
    const t0 = 10_000
    const entry = {
      start: 10_033,
      duration: 53,
      blocking: 20,
      renderStart: 10_070,
      styleAndLayoutStart: 10_076,
      scripts: [script],
    }
    const shifted = sampleLoaf(entry, t0)
    assert.equal(shifted.start, 33)
    assert.equal(shifted.renderStart, 70)
    assert.equal(shifted.styleAndLayoutStart, 76)
    assert.equal(styleAndLayoutMs(shifted), 10)
    // A relative start against the absolute styleAndLayoutStart is negative.
    // The fixture stays on the performance clock; the sample is what moves it.
    assert.ok(shifted.start + entry.duration - entry.styleAndLayoutStart < 0)

    const measured = measurementFrom(
      raw([[10_083.1, 50.1]], [entry], [{ start: 10_040, duration: 40.2 }]),
      t0,
    )
    assert.equal(measured.slow.length, 1)
    assert.equal(measured.slow[0].frame, 50.1)
    assert.equal(measured.slow[0].loaf.styleAndLayout, 10)
    assert.equal(measured.slow[0].longTask, 40)
    assert.equal(measured.slow[0].loaf.scripts[0].invoker, "pointerup")
    assert.equal(measured.slow[0].loaf.scripts[0].forcedLayout, 30)
  })

  it("reports no style and layout when the phase sentinel is zero", () => {
    const entry = {
      start: 10_030,
      duration: 53,
      blocking: 20,
      renderStart: 0,
      styleAndLayoutStart: 0,
      scripts: [],
    }
    const shifted = sampleLoaf(entry, 10_000)
    assert.equal(shifted.renderStart, null)
    assert.equal(shifted.styleAndLayoutStart, null)
    assert.equal(styleAndLayoutMs(shifted), null)

    const measured = measurementFrom(raw([[10_083, 53]], [entry]), 10_000)
    assert.equal(measured.slow[0].loaf.styleAndLayout, null)
  })

  it("keeps a phase that begins at the sample origin as a real zero", () => {
    const entry = {
      start: 10_000,
      duration: 53,
      blocking: 10,
      renderStart: 10_000,
      styleAndLayoutStart: 10_000,
      scripts: [],
    }
    const shifted = sampleLoaf(entry, 10_000)
    assert.equal(shifted.start, 0)
    assert.equal(shifted.renderStart, 0)
    assert.equal(shifted.styleAndLayoutStart, 0)
    assert.equal(styleAndLayoutMs(shifted), 53)

    const measured = measurementFrom(raw([[10_053, 53]], [entry]), 10_000)
    assert.equal(measured.slow[0].loaf.styleAndLayout, 53)
  })
})

it("attributes a sampled frame to work that began before the sample origin", () => {
  const entry = {
    start: 9_980,
    duration: 150,
    blocking: 70,
    renderStart: 10_000,
    styleAndLayoutStart: 10_120,
    scripts: [script],
  }
  const measured = measurementFrom(
    raw([[10_133, 133]], [entry], [{ start: 9_990, duration: 130 }]),
    10_000,
  )
  assert.equal(measured.maxFrame, 133)
  assert.equal(measured.over, 1)
  assert.equal(measured.slow[0].loaf.duration, 150)
  assert.equal(measured.slow[0].longTask, 130)
  assert.equal(measured.slow[0].loaf.styleAndLayout, 10)
})

it("retains attribution in the pre-origin part of the first sampled gap", () => {
  const entry = {
    start: 9_920,
    duration: 70,
    blocking: 10,
    renderStart: 9_970,
    styleAndLayoutStart: 9_980,
    scripts: [script],
  }
  const older = { ...entry, start: 9_850 }
  const measured = measurementFrom(
    raw(
      [[10_133, 153]],
      [older, entry],
      [
        { start: 9_800, duration: 50 },
        { start: 9_925, duration: 60 },
      ],
    ),
    10_000,
  )
  assert.equal(measured.maxFrame, 153)
  assert.equal(measured.over, 1)
  assert.equal(measured.slow[0].loaf.duration, 70)
  assert.equal(measured.slow[0].loaf.styleAndLayout, 10)
  assert.equal(measured.slow[0].longTask, 60)
})
