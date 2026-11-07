/**
 * The comparison interval and its verdict, on fixed inputs. The bootstrap
 * numbers below were produced by the percentile rule and mulberry32 at the
 * stated seed; a change to either moves them.
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import {
  bootstrapMedianInterval,
  comparisonVerdict,
  pairedDifferences,
  percentileIndexes,
} from "./compare.mjs"

describe("percentile indexes", () => {
  it("takes the 2.5% and 97.5% ranks of 10000 resamples", () => {
    assert.deepEqual(percentileIndexes(10_000, 0.95), { low: 250, high: 9749 })
  })
})

describe("paired differences", () => {
  it("subtracts each base run from the head run beside it", () => {
    assert.deepEqual(pairedDifferences([3, 1], [1, 4]), [2, -3])
    assert.deepEqual([3, 1], [3, 1])
  })

  it("refuses a pair that does not line up", () => {
    assert.throws(() => pairedDifferences([1], [1, 2]), /one head run/)
  })
})

describe("verdict", () => {
  it("calls a change only when the interval excludes zero", () => {
    assert.equal(comparisonVerdict(1, 2), "regression")
    assert.equal(comparisonVerdict(-3, -1), "win")
    assert.equal(comparisonVerdict(-1, 1), "no change")
    assert.equal(comparisonVerdict(0, 2), "no change")
    assert.equal(comparisonVerdict(-2, 0), "no change")
  })
})

describe("bootstrap median interval", () => {
  it("is empty when there are no differences", () => {
    assert.equal(bootstrapMedianInterval([]), null)
  })

  it("collapses a constant series to that constant", () => {
    const slower = bootstrapMedianInterval([5, 5, 5, 5], { resamples: 100, seed: 1 })
    assert.deepEqual(
      { median: slower.median, low: slower.low, high: slower.high },
      { median: 5, low: 5, high: 5 },
    )
    assert.equal(comparisonVerdict(slower.low, slower.high), "regression")

    const same = bootstrapMedianInterval([0, 0, 0], { resamples: 100, seed: 1 })
    assert.equal(comparisonVerdict(same.low, same.high), "no change")

    const faster = bootstrapMedianInterval([-3, -3, -3], { resamples: 100, seed: 1 })
    assert.deepEqual(
      { median: faster.median, low: faster.low, high: faster.high },
      { median: -3, low: -3, high: -3 },
    )
    assert.equal(comparisonVerdict(faster.low, faster.high), "win")
  })

  it("keeps a one-value interval and still requires it to exclude zero", () => {
    const slower = bootstrapMedianInterval([4], { resamples: 100, seed: 1 })
    assert.deepEqual([slower.low, slower.high], [4, 4])
    assert.equal(comparisonVerdict(slower.low, slower.high), "regression")
    const zero = bootstrapMedianInterval([0], { resamples: 100, seed: 1 })
    assert.equal(comparisonVerdict(zero.low, zero.high), "no change")
    const faster = bootstrapMedianInterval([-2], { resamples: 100, seed: 1 })
    assert.equal(comparisonVerdict(faster.low, faster.high), "win")
  })

  it("reproduces a mixed series from its seed and includes zero when the spread does", () => {
    const options = { resamples: 200, seed: 7, level: 0.95 }
    const values = [10, 0, -4, 8, 1]
    const interval = bootstrapMedianInterval(values, options)
    assert.deepEqual(interval, {
      median: 1,
      low: -4,
      high: 10,
      level: 0.95,
      resamples: 200,
      seed: 7,
      method: "percentile bootstrap of the median",
    })
    assert.equal(comparisonVerdict(interval.low, interval.high), "no change")
    assert.deepEqual(bootstrapMedianInterval(values, options), interval)
    assert.deepEqual(values, [10, 0, -4, 8, 1])
  })

  it("calls a clear positive shift a regression", () => {
    const interval = bootstrapMedianInterval([12, 14, 11, 15, 13], {
      resamples: 100,
      seed: 1,
    })
    assert.deepEqual(
      { median: interval.median, low: interval.low, high: interval.high },
      { median: 13, low: 11, high: 15 },
    )
    assert.equal(comparisonVerdict(interval.low, interval.high), "regression")
  })
})
