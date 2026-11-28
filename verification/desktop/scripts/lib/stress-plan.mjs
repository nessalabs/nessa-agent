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
  parentstops: {
    default: 1,
    what: "parent turns stopped in the window with running owned child and grandchild",
  },
  fails: { default: 1, what: "turns that fail, each followed by another message" },
  signIns: {
    default: 1,
    what: "turns the agent refuses for sign-in, each showing the sign-in card for where it ran and followed by another message",
  },
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
  closes: {
    default: 1,
    what: "conversations whose agent closes its own thread (close_thread) while the window shows it, parked once the turn ends, then talked to again",
  },
  provenances: {
    default: 1,
    what: "turns where a person's request reaches a thread through its parent's agent: the thread follows the message back with read_turn to the person's exact words, and the parent closes that thread with close_thread but not one it did not create",
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
    what: "conversations switched to the other agent from the composer's picker (staged, taken back, picked again, made by the next send), then forked from a row's More menu, and forked again into one id by three racing requests (one fork made, its copied turns recorded once), each talked to after",
  },
  unsigned: {
    default: 1,
    what: "conversations switched from the composer's picker to the other agent while it answers its opening as not signed in: the send is refused, the picker says the agent isn't signed in (the composer says nothing), no switch is recorded, and the next message runs on the agent it was on",
  },
  failedControls: {
    default: 1,
    what: "conversations whose model is changed while a turn runs and the agent fails the change (ending the turn with it): the change settles and the turn ends without anyone asking, the composer says nothing is unconfirmed, and the next message completes",
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
    what: "turns where an agent reads the window: beside another pane, the focused chat's panel (two tabs, the front tab, hidden and shown, then the other chat), then alone; the sidebar it folds parks once its slide ends and is reachable again when opened",
  },
  controls: {
    default: 1,
    what: "turns where an agent changes the window with its tools: another conversation opened beside, the sidebar and right panel closed and opened, the Agents overview and back, a missing panel tab refused, a pane focused, then closed, and closed again refused; each change on record in the window-control audit",
  },
  rails: {
    default: 1,
    what: 'times the side rail is turned on in Settings and opened around the traffic lights, a message sent, then closed and turned off, its toggle never moving; "nessa Studio" stays while a column holds the corner and goes over the panes alone; the chat\'s title never strays past either end of a toggle, and with the panes alone stands still',
  },
  waitingRows: {
    default: 1,
    what: "reviews waiting under Needs you as a plain session row, no answers or form beside it; opened from the row and answered in the chat, the row leaves Needs you",
  },
  streams: {
    default: 1,
    what: "replies streamed in over 512 small chunks and over 16 KB: the newest words are drawn live, still after the sidebar closes and opens, and a fresh read holds every word, nothing left out",
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
  mcpServers: {
    default: 1,
    what: "MCP servers added, edited (the stored variable kept when Replace is left empty) and removed from Settings, each followed by a message",
  },
  pastes: {
    default: 1,
    what: "images pasted into the composer as a screenshot is, shown as tiles, sent with the message and handed to the agent",
  },
  attaches: {
    default: 1,
    what: "a file and an image picked with the composer's Attach button, sent with the message: the image handed to the agent as one, the file placed where it works",
  },
  tables: {
    default: 1,
    what: "turns whose answer streams Markdown tables, one narrow and one wider than the reply, then code that stays dark with the OS in light mode, then maths; Words draws it, keeping each finished block as the rest streams",
  },
  traces: {
    default: 1,
    what: "turns whose tool calls the window draws as a waterfall, with their times and input kept by the gateway",
  },
  transcriptViews: {
    default: 1,
    what: "trace turns read in the compact transcript, chosen from the pane menu: only the reply and one Working line show, and the choice outlasts a reload",
  },
  notices: {
    default: 1,
    what: "reviews waiting in a conversation the window does not show: the host's notice opens it and is taken back once answered",
  },
  tunnels: {
    default: 1,
    what: "times the web tunnel is turned on in Settings, kept across a gateway restart with the same cloudflared, and turned off",
  },
  drifts: {
    default: 1,
    what: "gateway restarts with a changed agent PATH and TMPDIR, then a send completes; a focused Claude context with old history also survives a fingerprint change and cold restart before its first input",
  },
  upgrades: {
    default: 1,
    what: "gateway restarts onto conversation metadata turned back into an older build's file, which it upgrades and keeps, and onto every place record turned back into an older build's placement and binding files",
  },
  terminals: {
    default: 1,
    what: "terminals opened with ⌘J in the channel in front: a real shell in the pane that runs what is typed, listed in the session list and by the gateway, its details naming its folder, ended by its pane's ×",
  },
  resumes: {
    default: 1,
    what: "programs in terminals kept across a gateway restart (OSC 7577): resume is turned on in Settings › Terminal with a rule allowing one program, two terminals report a program each, and after the restart only the allowed one is back in its terminal, by the same id, running again; closing it and turning resume off forget it",
  },
  runs: {
    default: 1,
    what: "replies with shell fences the person runs: Run shows the output inline and ends with its exit and time, the stop icon interrupts a console fence's command, Run again starts anew and the terminal icon opens the run in a pane",
  },
  menus: {
    default: 1,
    what: "times the pane's ⋯ menu and the overview's filter are opened: rows lead with their icons, a chosen filter's check sits before its number, and a number picks it",
  },
  pickers: {
    default: 1,
    what: "times a model is starred and then chosen with its thinking level by one press of a bar in the model picker, opened on Starred, and the next message runs on it",
  },
  composers: {
    default: 1,
    what: "times the composer's row is read: the model is named without its family, Fast stands beside it, access before the hairline and Send, no project is offered, nothing stands under the card, the model picker's provider tiles are square, and in three short stacked panes an empty composer you are not in folds to a circle that opens it",
  },
  menuPanels: {
    default: 1,
    what: "times the menu-bar panel is opened over the run: its tabs are page dots after the chat's name, a message sent there wears the chat accent and its rim, and a chat archived from Messages leaves a row card whose Undo brings it back",
  },
  passphrases: {
    default: 1,
    what: "vault passphrase changes through Settings, each sealed and explicitly unlocked, followed by a conversation reply",
  },
  githubs: {
    default: 1,
    what: "times GitHub is connected through a fake GitHub, an agent reads through the broker without receiving the sign-in, then disconnected",
  },
  voices: {
    default: 1,
    what: "voice calls through a fake realtime provider: a key saved in Settings, Talk, spoken words sent tidied and the reply handed back, the microphone muted, a turn stopped by voice, and away from the window the floating pill: its mute and speaker switches, gone when the window is back in front, and End on it",
  },
  launches: {
    default: 1,
    what: "window reloads while the host is still starting the gateway: no startup screen, and the conversation listed as soon as it is up",
  },
  webhooks: {
    default: 1,
    what: "window turns captured by a disposable webhook with production loopback refusal and no attempt; positive TLS delivery is qualified separately",
  },
  sharedChats: {
    default: 1,
    what: "a granted chat opened from the follower's window: the transcript is read, a composed message is refused, and unshare takes the chat off the list",
  },
  peers: {
    default: 1,
    what: "paired peer gateways granted comment and drive that leave a note and send a turn the window shows",
  },
  huddles: {
    default: 1,
    what: "turns that show a voice app (the test server's talk board): its frame is allowed the microphone, it opens in the chat's panel as one tab, after Always allow its next message goes unasked, and Revoke in Settings › Access takes the trust back",
  },
  channels: {
    default: 1,
    what: "times channels are made from the sidebar's + (a refused folder, then one made for it; then a chosen folder), each talked to in a new session; the first then moves its new threads and main agent to Boat and back from its settings",
  },
  touchIds: {
    default: 1,
    what: "times the vault is unlocked with a fake Touch ID: the unlock dialog offers it ticked, the typed passphrase is kept, the next unlock is a fingerprint alone, a dismissed prompt leaves the form and its Use Touch ID button, changed fingerprints drop the copy and say so, and Settings turns it off, followed by a conversation reply",
  },
  swipes: {
    default: 1,
    what: "times two-finger sideways swipes over the chat, the sidebar and the session list are checked to move no side column and not the right panel (all open, or the sidebars folded), while a wide table and a wide code block still take their own swipes and a session row takes none",
  },
  styles: {
    default: 1,
    what: "times Threads are given the person's own words in the default channel's settings (saved only on Save): a new chat is told them, the main chat keeps the plain style it began with, and the choice is cleared after",
  },
  coordinators: {
    default: 1,
    what: "times a channel is made with its coordinator on (the first chat is its coordinator, told its job), switched off in its settings (a new chat there is an ordinary thread), and on again (the same coordinator chat); and one is made with it off",
  },
  asks: {
    default: 1,
    what: "times an agent asks its owner one decision in a conversation the window does not show: the row waits on the person, the host's notice says the question without the window moving, and the tapped choice is sent as the person's message, which takes the card and the notice away",
  },
  apps: {
    default: 1,
    what: "static apps made in Settings › Workspace › Apps: reviewed with a sandboxed preview, Cancel in the system's dialog installs nothing, Install keeps the exact reviewed package; opened in its own window with no network, stopped, then uninstalled while another app stays",
  },
  telemetries: {
    default: 1,
    what: "typed turns checked in the turn-origin audit (person, the window's client, not spoken) and as a turn span exported to a fake OpenTelemetry collector, with none of the message's words",
  },
  raises: {
    default: 1,
    what: "times the shield raises the main chat from Ask to Full while its turn waits on a review: the agent takes Full mid-turn, the open review is allowed by the raise, and the turn asks nothing more",
  },
  accessRequests: {
    default: 1,
    what: "times an agent asks to raise its chat's access (`request_access`): the red card says Nessa's question and the agent's reason, Approve raises the chat to Auto by the person's own access command before Approve is sent, and the card then lists what was allowed",
  },
  restores: {
    default: 1,
    what: "restored conversations opened in the window: one shows its saved history, and one whose source is gone refuses a send and keeps the draft",
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
export function plan(knobs, accessScenario = "full") {
  if (!["full", "ask-only"].includes(accessScenario))
    throw new Error(`unknown stress access scenario: ${accessScenario}`)
  const next = random(knobs.seed)
  const pick = (list) => list[Math.floor(next() * list.length)]
  const steps = []
  const add = (count, make) => {
    for (let i = 0; i < count; i += 1) steps.push(make())
  }
  add(knobs.stops, () => ({ kind: "stop" }))
  add(knobs.parentstops, () => ({ kind: "parent-stop" }))
  add(knobs.fails, () => ({ kind: "fail" }))
  add(knobs.usageLimits, () => ({ kind: "usageLimit" }))
  add(knobs.signIns, () => ({ kind: "signIn" }))
  add(knobs.restarts, () => ({ kind: "restart" }))
  add(knobs.backgroundTasks, () => ({ kind: "background" }))
  add(knobs.threads, () => ({ kind: "delegate" }))
  add(knobs.awaits, () => ({ kind: "await" }))
  add(knobs.watches, () => ({ kind: "watch" }))
  add(knobs.switches, () => ({ kind: "switch" }))
  add(knobs.inputFiles, () => ({ kind: "inputFiles" }))
  add(knobs.modelSwitches, () => ({ kind: "model" }))
  add(knobs.pickers, () => ({ kind: "picker" }))
  add(knobs.asides, () => ({ kind: "aside" }))
  add(knobs.large, () => ({ kind: "large" }))
  add(knobs.parks, () => ({ kind: "park" }))
  add(knobs.closes, () => ({ kind: "close" }))
  add(knobs.renames, () => ({ kind: "rename" }))
  add(knobs.handoffs, () => ({ kind: "handoff" }))
  add(knobs.unsigned, () => ({ kind: "unsigned" }))
  add(knobs.failedControls, () => ({ kind: "failedControl" }))
  add(knobs.deletes, () => ({ kind: "delete" }))
  add(knobs.fresh, () => ({ kind: "fresh" }))
  add(knobs.windows, () => ({ kind: "window" }))
  add(knobs.controls, () => ({ kind: "control" }))
  add(knobs.rails, () => ({ kind: "rail" }))
  add(knobs.panels, () => ({ kind: "panel" }))
  add(knobs.waitingRows, () => ({ kind: "waitingRow" }))
  add(knobs.streams, () => ({ kind: "stream" }))
  add(knobs.onboardings, () => ({ kind: "onboarding" }))
  add(knobs.linkings, () => ({ kind: "linking" }))
  add(knobs.mcpServers, () => ({ kind: "mcp" }))
  add(knobs.tables, () => ({ kind: "table" }))
  add(knobs.traces, () => ({ kind: "trace" }))
  add(knobs.transcriptViews, () => ({ kind: "transcriptView" }))
  add(knobs.notices, () => ({ kind: "notice" }))
  add(knobs.tunnels, () => ({ kind: "tunnel" }))
  add(knobs.drifts, () => ({ kind: "drift" }))
  add(knobs.upgrades, () => ({ kind: "upgrade" }))
  add(knobs.launches, () => ({ kind: "launch" }))
  add(knobs.menuPanels, () => ({ kind: "menuPanel" }))
  add(knobs.menus, () => ({ kind: "menu" }))
  add(knobs.terminals, () => ({ kind: "terminal" }))
  add(knobs.runs, () => ({ kind: "run" }))
  add(knobs.composers, () => ({ kind: "composer" }))
  add(knobs.githubs, () => ({ kind: "github" }))
  add(knobs.voices, () => ({ kind: "voice" }))
  add(knobs.passphrases, () => ({ kind: "passphrase" }))
  add(knobs.channels, () => ({ kind: "channel" }))
  add(knobs.huddles, () => ({ kind: "huddle" }))
  add(knobs.webhooks, () => ({ kind: "webhook" }))
  add(knobs.pastes, () => ({ kind: "paste" }))
  add(knobs.attaches, () => ({ kind: "attach" }))
  if (knobs.long > 0) steps.push({ kind: "long", turns: knobs.long })
  if (knobs.reviews > 0) steps.push({ kind: "reviews" })
  while (steps.length < knobs.rounds)
    steps.push({ kind: next() < 0.5 ? "reply" : "permission" })
  // Fisher–Yates, seeded. Restored conversations and swipes are added after
  // it so a new count does not reshuffle the steps a seed already fixed.
  for (let i = steps.length - 1; i > 0; i -= 1) {
    const j = Math.floor(next() * (i + 1))
    ;[steps[i], steps[j]] = [steps[j], steps[i]]
  }
  for (let slot = 0; slot < knobs.restores; slot += 1)
    steps.push({ kind: "restore", slot })
  // After the shuffle, with restores: a new default must not reshuffle a seed.
  for (let slot = 0; slot < knobs.peers; slot += 1) steps.push({ kind: "peer" })
  add(knobs.swipes, () => ({ kind: "swipe" }))
  // After the shuffle, so the new knob does not reshuffle a seed.
  add(knobs.provenances, () => ({ kind: "provenance" }))
  add(knobs.touchIds, () => ({ kind: "touchId" }))
  add(knobs.styles, () => ({ kind: "style" }))
  add(knobs.coordinators, () => ({ kind: "coordinator" }))
  add(knobs.asks, () => ({ kind: "ask" }))
  add(knobs.accessRequests, () => ({ kind: "accessRequest" }))
  // After the shuffle, so the new knob does not reshuffle a seed.
  add(knobs.raises, () => ({ kind: "raise" }))
  // After the shuffle, so the new knob does not reshuffle a seed.
  add(knobs.resumes, () => ({ kind: "resume" }))
  // After the shuffle, so the new knob does not reshuffle a seed.
  add(knobs.telemetries, () => ({ kind: "telemetry" }))
  // After the shuffle, so the new knob does not reshuffle a seed.
  add(knobs.apps, () => ({ kind: "apps" }))
  if (knobs.pairedCatalogues > 0)
    steps.push({ kind: "pairedCatalogue", cycles: knobs.pairedCatalogues })
  for (let slot = 0; slot < knobs.sharedChats; slot += 1)
    steps.push({ kind: "sharedChat", slot })
  // Preset checks keep their original model. Positive model changes (a
  // `model` step, and a `picker` step's bar) follow that work, using Ask
  // before their review; chaos keeps concurrent coverage.
  // Handoffs come last of all (a refused one too): a switch stays in the metadata, and no older
  // build's file holds one, so the `upgrade` step runs before it.
  const last = (step) =>
    step.kind === "handoff" || step.kind === "unsigned"
      ? 2
      : step.kind === "model" || step.kind === "picker" || step.kind === "failedControl"
        ? 1
        : 0
  const sequence = [0, 1, 2].flatMap((rank) =>
    steps.filter((step) => last(step) === rank),
  )
  let mode = "ask"
  const out = []
  for (const step of sequence) {
    if (step.kind === "switch") {
      const to = pick(MODES.filter((each) => each !== mode))
      const during = next() < 0.5
      if (accessScenario === "ask-only")
        out.push({ kind: "refused-access", from: mode, to: "full", during })
      else {
        out.push({ kind: "switch", from: mode, to, during })
        mode = to
      }
    } else if (step.kind === "permission") {
      out.push({ ...step, answer: next() < 0.7 ? "allow" : "deny", mode })
    } else if (
      step.kind === "reviews" ||
      step.kind === "large" ||
      step.kind === "waitingRow" ||
      step.kind === "model"
    ) {
      // These are about cards being shown, so they run where the agent asks.
      if (mode === "full" || (step.kind === "model" && mode !== "ask")) {
        out.push({ kind: "switch", from: mode, to: "ask", during: false })
        mode = "ask"
      }
      out.push({ ...step, answer: next() < 0.7 ? "allow" : "deny", mode })
    } else if (step.kind === "raise") {
      // Where the model offers no Full there is nothing to raise to.
      if (accessScenario === "ask-only") continue
      // Raised from Ask, where the agent asks; Full afterwards.
      if (mode !== "ask") {
        out.push({ kind: "switch", from: mode, to: "ask", during: false })
        mode = "ask"
      }
      out.push({ ...step, from: "ask", to: "full", mode: "ask" })
      mode = "full"
    } else if (step.kind === "restart") {
      out.push({ kind: "restart", signal: next() < 0.5 ? "SIGKILL" : "SIGTERM" })
    } else {
      out.push(step)
    }
    if (step.kind === "fail" || step.kind === "usageLimit" || step.kind === "signIn")
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
        step.kind === "switch" || step.kind === "refused-access" || step.kind === "raise"
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

/** Select existing planned steps without changing a single knob or deadline.
 * The unfiltered plan remains the full qualification; a selected run reports
 * its scope explicitly and cannot stand in for the unselected feature checks. */
export function selectedPlan(knobs, only, accessScenario = "full") {
  const planned = plan(knobs, accessScenario)
  if (!only) return planned
  const selected = new Set(
    only
      .split(",")
      .map((name) => name.trim())
      .filter(Boolean),
  )
  const available = new Set(planned.map((step) => step.kind))
  const unknown = [...selected].filter((kind) => !available.has(kind))
  if (selected.size === 0 || unknown.length)
    throw new Error(`No planned stress step matches: ${unknown.join(",") || only}`)
  return planned.filter((step) => selected.has(step.kind))
}
