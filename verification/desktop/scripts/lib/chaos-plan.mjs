/**
 * The conversation chaos check's numbers, the seeded actions they make, and
 * how a failing list of actions is cut down to the shortest one that still
 * fails (`conversation-chaos.mjs`). Pure: the same numbers and seed give the
 * same actions on every machine.
 *
 * An action names what is done (`kind`), to which conversation (`slot`),
 * from which connected client (`client`), and in which wave. The actions of
 * one wave run at the same time; the next wave starts when they have all
 * answered. Which of them lands first is up to the gateway, so a seed fixes
 * what is tried, not the order it lands in: a failure that depends on that
 * order can need a few runs of the same seed to show again.
 */
import { random } from "./stress-plan.mjs"

export { random }

/** Every number the check takes: its default, and what it counts. */
export const KNOBS = {
  seed: { default: 1, what: "the actions' seed; the same seed gives the same actions" },
  waves: { default: 30, what: "waves of actions" },
  width: { default: 6, what: "actions in one wave, all sent at the same time" },
  conversations: { default: 4, what: "conversations the actions are spread over" },
  clients: {
    default: 3,
    what: "connected clients sending them, as windows and devices of one person",
  },
  followers: {
    default: 2,
    what: "other clients following every conversation's events, view and list",
  },
  restarts: {
    default: 2,
    what: "gateway crashes or shutdowns among the actions, at most one a wave",
  },
  reviews: { default: 3, what: "reviews one many-reviews turn asks in a row" },
  live: {
    default: 0,
    what: "live agents the gateway keeps open at once, idle ones let go past it; 0 keeps its own",
  },
  shrink: {
    default: 8,
    what: "runs at most spent cutting a failing list of actions down; 0 skips it",
  },
}

/** The numbers, from flags, then `NESSA_CHAOS_*`, then the defaults. A bad one throws. */
export function knobsFrom(options = {}, env = process.env) {
  const knobs = {}
  for (const [name, knob] of Object.entries(KNOBS)) {
    const raw = options[name] ?? env[`NESSA_CHAOS_${name.toUpperCase()}`]
    const value = raw === undefined || raw === "" ? knob.default : Number(raw)
    if (!Number.isSafeInteger(value) || value < 0)
      throw new Error(`${name} is a whole number, 0 or more: ${JSON.stringify(raw)}`)
    knobs[name] = value
  }
  if (knobs.conversations < 1) throw new Error("conversations is 1 or more")
  if (knobs.clients < 1) throw new Error("clients is 1 or more")
  return knobs
}

/**
 * What a message asks the agent for, by the stress scenario's phrases
 * (`scripts/mcp-test-server/scenarios/stress.mjs`): a reply, one review,
 * several reviews, a pause, a turn that runs until it is stopped, a failure,
 * a thread handed work, and a thread read with more reads than one
 * conversation may hold, or owned descendants waiting for parent Stop.
 */
export const ASKS = [
  "reply",
  "permission",
  "reviews",
  "pause",
  "wait",
  "fail",
  "delegate",
  "watch",
  "parentStop",
]
/** What a steering message asks: only turns that end on their own. */
const STEERS = ["reply", "permission", "pause"]
const MODES = ["ask", "auto", "full"]
/**
 * What a rename types: a plain title, one with runs of whitespace and a
 * control character to fold, and one longer than a title may be.
 */
const TITLES = ["plain", "folded", "long"]

/**
 * Each kind of action and how often it is drawn. `restart` is not drawn; its
 * number places it.
 */
const WEIGHTS = {
  send: 30,
  steer: 8,
  note: 5,
  answer: 16,
  mode: 6,
  stop: 7,
  remove: 4,
  reorder: 3,
  close: 2,
  archive: 3,
  rename: 3,
  read: 8,
}

function weighted(next) {
  const total = Object.values(WEIGHTS).reduce((sum, each) => sum + each, 0)
  let roll = next() * total
  for (const [kind, weight] of Object.entries(WEIGHTS)) {
    roll -= weight
    if (roll < 0) return kind
  }
  return "send"
}

/** The actions for these numbers, in wave order; each numbered from 1. */
export function actions(knobs) {
  const next = random(knobs.seed)
  const pick = (list) => list[Math.floor(next() * list.length)]
  const out = []
  // Restarts land in waves spread over the run, never the first or last.
  const restartWaves = new Set()
  const restarts = Math.min(knobs.restarts, Math.max(knobs.waves - 2, 0))
  while (restartWaves.size < restarts)
    restartWaves.add(1 + Math.floor(next() * (knobs.waves - 2)))
  for (let wave = 0; wave < knobs.waves; wave += 1) {
    if (restartWaves.has(wave))
      out.push({ wave, kind: "restart", signal: next() < 0.5 ? "SIGKILL" : "SIGTERM" })
    for (let n = 0; n < knobs.width; n += 1) {
      const kind = weighted(next)
      const action = {
        wave,
        kind,
        slot: Math.floor(next() * knobs.conversations),
        client: Math.floor(next() * knobs.clients),
      }
      if (kind === "send") action.ask = pick(ASKS)
      else if (kind === "steer") action.ask = pick(STEERS)
      else if (kind === "note")
        action.delivery = next() < 0.5 ? "record_only" : "next_turn"
      else if (kind === "answer")
        action.answer = pick(["allow", "allow", "allow", "deny", "withdraw"])
      else if (kind === "mode") action.to = pick(MODES)
      else if (kind === "stop") action.which = next() < 0.6 ? "running" : "queued"
      else if (kind === "archive") action.undo = next() < 0.5
      else if (kind === "rename") action.title = pick(TITLES)
      out.push(action)
    }
  }
  return out.map((action, index) => ({ index: index + 1, ...action }))
}

/** One line per action, for the log and a failure's report. */
export function describe(list) {
  return list
    .map((action) => {
      const extra =
        action.ask ??
        action.delivery ??
        action.answer ??
        action.to ??
        action.which ??
        action.signal ??
        action.title ??
        (action.kind === "archive" ? (action.undo ? "undo" : "") : "")
      const where =
        action.kind === "restart" ? "" : ` c${action.slot} by ${action.client}`
      return `${action.index}. w${action.wave} ${action.kind}${extra ? ` ${extra}` : ""}${where}`
    })
    .join("\n")
}

/**
 * The shortest list of `list`'s actions found for which `fails(list)` still
 * resolves true, trying at most `budget` lists (delta debugging: drop halves,
 * then quarters, and so on, keeping any cut that still fails). The actions
 * keep their order and their waves. Resolves `{ list, tried }`; `list` is the
 * input itself when no cut failed. `kept(list)` hears each shorter list as it
 * is found, so a run stopped part way still leaves its best cut.
 */
export async function shrink(list, fails, budget, kept = () => {}) {
  let current = list
  let parts = 2
  let tried = 0
  while (current.length > 1 && tried < budget) {
    const size = Math.ceil(current.length / parts)
    let cut = false
    for (let start = 0; start < current.length && tried < budget; start += size) {
      const rest = [...current.slice(0, start), ...current.slice(start + size)]
      if (rest.length === 0) continue
      tried += 1
      if (await fails(rest)) {
        current = rest
        kept(current)
        parts = Math.max(parts - 1, 2)
        cut = true
        break
      }
    }
    if (!cut) {
      if (size === 1) break
      parts = Math.min(parts * 2, current.length)
    }
  }
  return { list: current, tried }
}
