/**
 * A subagent's tagline: one line picked from its seed, so the same subagent
 * always says the same thing. It is decoration. It says nothing about what
 * the subagent is doing (`tagline.test.ts` holds the pick stable, and that
 * every line is reachable).
 */
const lines = [
  "Saving the world, one diff at a time",
  "I just want to retire",
  "Was told there would be snacks",
  "Professionally overthinking it",
  "Here for the green checkmarks",
  "Running on vibes and unit tests",
  "Has opinions about naming",
  "Allergic to flaky tests",
  "Will refactor for food",
  "Believes every bug is a feature request",
  "Reads the docs so you don't have to",
  "Quietly judging your regexes",
  "One more run, then I'm done. Probably.",
  "Measured twice, cut once, reverted once",
  "In it for the long context",
  "Fluent in stack traces",
  "Suspicious of anything that passes first try",
  "Keeps a tidy diff",
  "Here to make the number go up",
  "Collects edge cases like stamps",
  "Would rather be writing tests",
  "Has seen things in production",
  "Low latency, high hopes",
  "Chasing the last half point",
] as const

/** Every line a seed can pick. The test that each is reachable reads this. */
export const taglines: readonly string[] = lines

/** A small stable hash of a string's code units. */
function hash(text: string): number {
  let value = 2166136261
  for (let index = 0; index < text.length; index += 1)
    value = Math.imul(value ^ text.charCodeAt(index), 16777619)
  return value >>> 0
}

/** The line a subagent with this seed says; the same seed always returns it. */
export function tagline(seed: string): string {
  return lines[hash(seed) % lines.length]
}
