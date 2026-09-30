/**
 * A subagent's character: one line it says about itself, picked from its seed,
 * so the same subagent always says the same thing and two beside each other
 * rarely do. Nothing reads meaning into it; it is there so a swarm of agents
 * reads as a crew rather than a list.
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

/** A small, stable hash of a string. */
function hash(text: string): number {
  let value = 2166136261
  for (let index = 0; index < text.length; index += 1)
    value = Math.imul(value ^ text.charCodeAt(index), 16777619)
  return value >>> 0
}

/** The line a subagent with this seed says about itself; always the same one. */
export function characterLine(seed: string): string {
  return lines[hash(seed) % lines.length]
}
