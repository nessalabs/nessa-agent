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
  backgroundTasks: {
    default: 1,
    what: "background commands outliving their calling turn and settling the original tool row",
  },
  inputFiles: {
    default: 1,
    what: "PDF, ZIP and text inputs uploaded by another surface, retained and drawn in the conversation",
  },
  pairedCatalogues: {
    default: 1,
    what: "real native paired catalogue cycles through the protected product child",
  },
  modelSwitches: {
    default: 1,
    what: "same-provider model choices through the actual composer picker, applied by the next send",
  },
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
  usageLimits: {
    default: 1,
    what: "turns a usage limit ends, each showing the card and followed by another message",
  },
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
  awaits: {
    default: 1,
    what: "turns that hand work to another thread and wait for its reply in one subscribe call",
  },
  watches: {
    default: 1,
    what: "turns where the agent follows a thread with more reads at once than one conversation may hold",
  },
  followers: {
    default: 2,
    what: "other clients following every conversation's events, view and list, stopping and following again as they go",
  },
  parks: {
    default: 1,
    what: "conversations left idle until the gateway parks their agent, then talked to again",
  },
  renames: {
    default: 1,
    what: "conversations renamed from a row's More menu, then talked to under the new title",
  },
  handoffs: {
    default: 1,
    what: "conversations switched to the other agent from the composer's picker (staged, taken back, picked again, made by the next send), then forked from a row's More menu, each talked to after",
  },
  deletes: {
    default: 1,
    what: "conversations deleted from a row's More menu, once asked",
  },
  fresh: {
    default: 2,
    what: "conversations started from a new session's home in the window, their first message sent there",
  },
  windows: {
    default: 1,
    what: "turns where an agent reads the window: beside another pane, then alone",
  },
  rails: {
    default: 1,
    what: 'times the side rail is turned on in Settings and opened around the traffic lights, a message sent, then closed and turned off, its toggle never moving; "nessa Studio" stays while a column holds the corner and goes over the panes alone',
  },
  listAnswers: {
    default: 1,
    what: "reviews answered from the session list's Needs you card: it reads the transcript card's command, and its answer settles the turn",
  },
  panels: {
    default: 1,
    what: "times the right panel is opened with ⌥⌘B beside a conversation, a message sent, expanded for that chat alone (its own tabs or note, no chat tabs; ⌘N docks it and the new session opens in sight), left with Escape, then closed",
  },
  onboardings: {
    default: 1,
    what: "times setup is replayed from Settings and walked forward, back with its Back button, and left with its Close button",
  },
  linkings: {
    default: 1,
    what: "times native linking is turned on and off again from Settings, each restarting the gateway, with a code opened by Pair a device while it is on",
  },
  tables: {
    default: 1,
    what: "turns whose answer streams Markdown tables, one narrow and one wider than the reply",
  },
  traces: {
    default: 1,
    what: "turns whose tool calls the window draws as a waterfall, with their times and input kept by the gateway",
  },
  notices: {
    default: 1,
    what: "reviews waiting in a conversation the window does not show: the host's notice opens it and is taken back once answered",
  },
  pools: {
    default: 1,
    what: "times a conversation placed on any worker moves to another when its worker drains",
  },
  tunnels: {
    default: 1,
    what: "times the web tunnel is turned on in Settings, kept across a gateway restart, dialed through by a paired worker, and turned off",
  },
  drifts: {
    default: 1,
    what: "gateway restarts with a changed agent PATH and TMPDIR, then a send to the existing conversation completes",
  },
  upgrades: {
    default: 1,
    what: "gateway restarts onto conversation metadata turned back into an older build's file, which it upgrades and keeps",
  },
  menus: {
    default: 1,
    what: "times the pane's ⋯ menu and the overview's filter are opened: rows lead with their icons, a chosen filter's check sits before its number, and a number picks it",
  },
  menuPanels: {
    default: 1,
    what: "times the menu-bar panel is opened over the run: its tabs are page dots after the chat's name, a message sent there wears the chat accent and its rim, and a chat archived from Messages leaves a row card whose Undo brings it back",
  },
  githubs: {
    default: 1,
    what: "times GitHub is connected from Settings through a fake GitHub, an agent started after it holds the sign-in, then disconnected",
  },
  launches: {
    default: 1,
    what: "window reloads while the host is still starting the gateway: no startup screen, and the conversation listed as soon as it is up",
  },
  webhooks: {
    default: 1,
    what: "window turns captured by a disposable webhook with production loopback refusal and no attempt; positive TLS delivery is qualified separately",
  },
  channels: {
    default: 1,
    what: "times channels are made from the sidebar's + (a refused folder, then one made for it; then a chosen folder), each talked to in a new session",
  },
  long: {
    default: 30,
    what: "turns of long streamed answers one conversation takes before it is talked to in the window",
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
  add(knobs.usageLimits, () => ({ kind: "usageLimit" }))
  add(knobs.restarts, () => ({ kind: "restart" }))
  add(knobs.backgroundTasks, () => ({ kind: "background" }))
  add(knobs.threads, () => ({ kind: "delegate" }))
  add(knobs.awaits, () => ({ kind: "await" }))
  add(knobs.watches, () => ({ kind: "watch" }))
  add(knobs.switches, () => ({ kind: "switch" }))
  add(knobs.inputFiles, () => ({ kind: "inputFiles" }))
  add(knobs.modelSwitches, () => ({ kind: "model" }))
  add(knobs.asides, () => ({ kind: "aside" }))
  add(knobs.large, () => ({ kind: "large" }))
  add(knobs.parks, () => ({ kind: "park" }))
  add(knobs.renames, () => ({ kind: "rename" }))
  add(knobs.handoffs, () => ({ kind: "handoff" }))
  add(knobs.deletes, () => ({ kind: "delete" }))
  add(knobs.fresh, () => ({ kind: "fresh" }))
  add(knobs.windows, () => ({ kind: "window" }))
  add(knobs.rails, () => ({ kind: "rail" }))
  add(knobs.panels, () => ({ kind: "panel" }))
  add(knobs.listAnswers, () => ({ kind: "listAnswer" }))
  add(knobs.onboardings, () => ({ kind: "onboarding" }))
  add(knobs.linkings, () => ({ kind: "linking" }))
  add(knobs.tables, () => ({ kind: "table" }))
  add(knobs.traces, () => ({ kind: "trace" }))
  add(knobs.notices, () => ({ kind: "notice" }))
  add(knobs.pools, () => ({ kind: "pool" }))
  add(knobs.tunnels, () => ({ kind: "tunnel" }))
  add(knobs.drifts, () => ({ kind: "drift" }))
  add(knobs.upgrades, () => ({ kind: "upgrade" }))
  add(knobs.launches, () => ({ kind: "launch" }))
  add(knobs.menuPanels, () => ({ kind: "menuPanel" }))
  add(knobs.menus, () => ({ kind: "menu" }))
  add(knobs.githubs, () => ({ kind: "github" }))
  add(knobs.channels, () => ({ kind: "channel" }))
  add(knobs.webhooks, () => ({ kind: "webhook" }))
  if (knobs.long > 0) steps.push({ kind: "long", turns: knobs.long })
  if (knobs.reviews > 0) steps.push({ kind: "reviews" })
  while (steps.length < knobs.rounds)
    steps.push({ kind: next() < 0.5 ? "reply" : "permission" })
  // Fisher–Yates, seeded.
  for (let i = steps.length - 1; i > 0; i -= 1) {
    const j = Math.floor(next() * (i + 1))
    ;[steps[i], steps[j]] = [steps[j], steps[i]]
  }
  if (knobs.pairedCatalogues > 0)
    steps.push({ kind: "pairedCatalogue", cycles: knobs.pairedCatalogues })
  // Preset checks keep their original model. Positive model changes follow
  // that work, using Ask before their review; chaos keeps concurrent coverage.
  // Handoffs come last of all: a switch stays in the metadata, and no older
  // build's file holds one, so the `upgrade` step runs before it.
  const last = (step) => (step.kind === "handoff" ? 2 : step.kind === "model" ? 1 : 0)
  const sequence = [0, 1, 2].flatMap((rank) =>
    steps.filter((step) => last(step) === rank),
  )
  let mode = "ask"
  const out = []
  for (const step of sequence) {
    if (step.kind === "switch") {
      const to = pick(MODES.filter((each) => each !== mode))
      out.push({ kind: "switch", from: mode, to, during: next() < 0.5 })
      mode = to
    } else if (step.kind === "permission") {
      out.push({ ...step, answer: next() < 0.7 ? "allow" : "deny", mode })
    } else if (
      step.kind === "reviews" ||
      step.kind === "large" ||
      step.kind === "listAnswer" ||
      step.kind === "model"
    ) {
      // These are about cards being shown, so they run where the agent asks.
      if (mode === "full" || (step.kind === "model" && mode !== "ask")) {
        out.push({ kind: "switch", from: mode, to: "ask", during: false })
        mode = "ask"
      }
      out.push({ ...step, answer: next() < 0.7 ? "allow" : "deny", mode })
    } else if (step.kind === "restart") {
      out.push({ kind: "restart", signal: next() < 0.5 ? "SIGKILL" : "SIGTERM" })
    } else {
      out.push(step)
    }
    if (step.kind === "fail" || step.kind === "usageLimit")
      out.push({ kind: "reply", after: step.kind })
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
            : step.kind === "long"
              ? ` ${step.turns} turns`
              : step.answer
                ? ` ${step.answer} (${step.mode})`
                : step.after
                  ? ` after ${step.after}`
                  : ""
      return `${step.index}. ${step.kind}${extra}`
    })
    .join("\n")
}
