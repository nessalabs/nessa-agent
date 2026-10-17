/**
 * The conversation stress check's numbers and the seeded plan they make
 * (`conversation-stress.mjs`). Pure: the same numbers and seed give the same
 * plan on every machine, so a failure found at any size is replayed by
 * passing that run's numbers again.
 *
 * Each number is a flag (`--rounds 40`) or, when no flag names it, an
 * environment variable (`NESSA_STRESS_ROUNDS=40`), else its default. The
 * defaults are the pull request's run: every kind of step at least once, in
 * about two minutes. The load run raises them.
 */

/** Every number the check takes: its default, and what it counts. */
export const KNOBS = {
  seed: { default: 1, what: "the plan's seed; the same seed gives the same plan" },
  rounds: {
    default: 16,
    what: "steps on the main conversation, at least; the required ones below count toward it",
  },
  asides: {
    default: 2,
    what: "other conversations opened and talked to in the window between rounds",
  },
  parallel: {
    default: 2,
    what: "background conversations talking through the gateway at the same time",
  },
  turns: { default: 3, what: "turns each background conversation takes" },
  switches: { default: 3, what: "access mode changes from the composer's shield" },
  stops: { default: 2, what: "turns stopped with the composer's Stop" },
  fails: { default: 1, what: "turns that fail, each followed by another message" },
  reviews: {
    default: 20,
    what: "reviews one turn asks in a row (past the old limit of 16)",
  },
  large: {
    default: 1,
    what: "turns that ask one review larger than the old 16 KB limit",
  },
  restarts: {
    default: 1,
    what: "gateway restarts during a running turn (crash or shutdown)",
  },
  threads: {
    default: 1,
    what: "turns that hand work to another thread and wait for its report",
  },
}

/** The numbers, from flags, then `NESSA_STRESS_*`, then the defaults. A bad one throws. */
export function knobsFrom(options = {}, env = process.env) {
  const knobs = {}
  for (const [name, knob] of Object.entries(KNOBS)) {
    const raw = options[name] ?? env[`NESSA_STRESS_${name.toUpperCase()}`]
    const value = raw === undefined || raw === "" ? knob.default : Number(raw)
    if (!Number.isSafeInteger(value) || value < 0)
      throw new Error(`${name} is a whole number, 0 or more: ${JSON.stringify(raw)}`)
    knobs[name] = value
  }
  return knobs
}

/** A seeded generator of numbers in [0, 1) (mulberry32). */
export function random(seed) {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const MODES = ["ask", "auto", "full"]

/**
 * The steps on the main conversation, in order. Each required step appears
 * as many times as its number says; `reply` and `permission` fill the rest
 * up to `rounds`. A failed turn is always followed by a reply, since the
 * message after a failure is what broke before, and a switch by a
 * permission turn, since whether the agent asks is how the switch is seen. A mode switch names the mode
 * it picks (never the one in force) and whether it is picked while a turn
 * runs. A restart names its signal: SIGKILL is a crash, SIGTERM a shutdown.
 * The many-reviews and large-review turns are about cards being shown, so a
 * switch back to ask comes first when full access is in force.
 */
export function plan(knobs) {
  const next = random(knobs.seed)
  const pick = (list) => list[Math.floor(next() * list.length)]
  const steps = []
  const add = (count, make) => {
    for (let i = 0; i < count; i += 1) steps.push(make())
  }
  add(knobs.stops, () => ({ kind: "stop" }))
  add(knobs.fails, () => ({ kind: "fail" }))
  add(knobs.restarts, () => ({ kind: "restart" }))
  add(knobs.threads, () => ({ kind: "delegate" }))
  add(knobs.switches, () => ({ kind: "switch" }))
  add(knobs.asides, () => ({ kind: "aside" }))
  add(knobs.large, () => ({ kind: "large" }))
  if (knobs.reviews > 0) steps.push({ kind: "reviews" })
  while (steps.length < knobs.rounds)
    steps.push({ kind: next() < 0.5 ? "reply" : "permission" })
  // Fisher–Yates, seeded.
  for (let i = steps.length - 1; i > 0; i -= 1) {
    const j = Math.floor(next() * (i + 1))
    ;[steps[i], steps[j]] = [steps[j], steps[i]]
  }
  let mode = "ask"
  const out = []
  for (const step of steps) {
    if (step.kind === "switch") {
      const to = pick(MODES.filter((each) => each !== mode))
      out.push({ kind: "switch", from: mode, to, during: next() < 0.5 })
      mode = to
    } else if (step.kind === "permission") {
      out.push({ ...step, answer: next() < 0.7 ? "allow" : "deny", mode })
    } else if (step.kind === "reviews" || step.kind === "large") {
      // These are about cards being shown, so they run where the agent asks.
      if (mode === "full") {
        out.push({ kind: "switch", from: mode, to: "ask", during: false })
        mode = "ask"
      }
      out.push({ ...step, answer: next() < 0.7 ? "allow" : "deny", mode })
    } else if (step.kind === "restart") {
      out.push({ kind: "restart", signal: next() < 0.5 ? "SIGKILL" : "SIGTERM" })
    } else {
      out.push(step)
    }
    if (step.kind === "fail") out.push({ kind: "reply", after: "fail" })
    // Whether the agent asks is how a switch is seen to have reached it.
    if (step.kind === "switch")
      out.push({
        kind: "permission",
        answer: next() < 0.7 ? "allow" : "deny",
        mode,
        after: "switch",
      })
  }
  return out.map((step, index) => ({ index: index + 1, ...step }))
}

/** One line per step, for the log and the pull request summary. */
export function describe(steps) {
  return steps
    .map((step) => {
      const extra =
        step.kind === "switch"
          ? ` ${step.from}→${step.to}${step.during ? " during a turn" : ""}`
          : step.kind === "restart"
            ? ` ${step.signal}`
            : step.answer
              ? ` ${step.answer} (${step.mode})`
              : step.after
                ? ` after ${step.after}`
                : ""
      return `${step.index}. ${step.kind}${extra}`
    })
    .join("\n")
}
