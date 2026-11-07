/**
 * Paired comparison of two builds measured on the same host.
 *
 * Each pair is one base run and the head run taken immediately after it.
 * The difference is head minus base, in milliseconds of the run's longest
 * frame: a positive number means the head build was slower. The reported
 * interval is a percentile bootstrap of the median of those differences.
 * A row is a regression or a win only when that interval excludes zero.
 *
 * The resample generator is mulberry32. The same samples and seed always
 * produce the same interval (`compare.test.mjs`).
 */
import { median } from "./perf.mjs"

export const comparisonSeed = 1
export const comparisonResamples = 10_000
export const comparisonLevel = 0.95

/** Indexes into a sorted list of `resamples` bootstrap medians. */
export function percentileIndexes(resamples, level = comparisonLevel) {
  if (!Number.isInteger(resamples) || resamples < 1)
    throw new Error("resamples must be a positive integer")
  if (!(level > 0 && level < 1)) throw new Error("level must be between 0 and 1")
  const alpha = (1 - level) / 2
  return {
    low: Math.floor(alpha * resamples),
    high: Math.min(resamples - 1, Math.ceil((1 - alpha) * resamples) - 1),
  }
}

/** One difference per pair. Lengths must match; the inputs are not reordered. */
export function pairedDifferences(head, base) {
  if (head.length !== base.length)
    throw new Error("a comparison needs one head run for each base run")
  return head.map((value, index) => value - base[index])
}

/**
 * 95% percentile bootstrap of the median, unless `level` says otherwise.
 * `seed` fixes the resamples. An empty list has no interval.
 */
export function bootstrapMedianInterval(values, options = {}) {
  if (values.length === 0) return null
  const resamples = options.resamples ?? comparisonResamples
  const seed = options.seed ?? comparisonSeed
  const level = options.level ?? comparisonLevel
  const { low: lowIndex, high: highIndex } = percentileIndexes(resamples, level)
  const point = median(values)
  const random = mulberry32(seed)
  const stats = new Array(resamples)
  for (let i = 0; i < resamples; i++) {
    const draw = new Array(values.length)
    for (let j = 0; j < values.length; j++)
      draw[j] = values[Math.floor(random() * values.length)]
    stats[i] = median(draw)
  }
  stats.sort((a, b) => a - b)
  return {
    median: point,
    low: stats[lowIndex],
    high: stats[highIndex],
    level,
    resamples,
    seed,
    method: "percentile bootstrap of the median",
  }
}

/**
 * `low` and `high` are the interval on head minus base.
 * Positive and excluding zero is a regression. Negative and excluding zero
 * is a win. An interval that contains zero, including an endpoint on zero,
 * is no change.
 */
export function comparisonVerdict(low, high) {
  if (low > 0) return "regression"
  if (high < 0) return "win"
  return "no change"
}

function mulberry32(seed) {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}
