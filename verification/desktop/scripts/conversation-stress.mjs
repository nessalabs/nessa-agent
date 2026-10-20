#!/usr/bin/env node
/**
 * The conversation stress check: a person using one conversation for many
 * rounds through the real gateway and the desktop window, with the scripted
 * agent standing in for the model. Seeded, so a failing run is replayed by
 * its numbers.
 *
 * Every message on the main conversation is typed into the window's composer
 * and sent with its Send button, never through the client library, because
 * the client always makes fresh request ids and so hid the desktop's own
 * send bugs. Between rounds the plan (`lib/stress-plan.mjs`) mixes in:
 *
 *   reply       a plain turn; the window shows its echo
 *   permission  the agent asks; a card is answered in the window — or, under
 *               full access, nothing is asked and the tool runs
 *   reviews     one turn asks `--reviews` reviews in a row, each a card
 *   large       one review larger than the old 16 KB display limit
 *   stop        Send turns into Stop while the turn runs; Stop cancels it
 *   fail        the turn fails; the next message is sent in the same conversation
 *   switch      the access shield picks another mode, sometimes mid-turn,
 *               where the change waits for the turn to end
 *   restart     the gateway is killed (crash) or stopped (shutdown) mid-turn
 *               and started again; the turn is resumed and the window follows
 *   delegate    the agent creates a thread and messages it; the thread's
 *               report comes back as a turn of its own
 *   await       the agent messages a thread and waits for it with one
 *               subscribe call naming the sent turn: the thread's reply comes
 *               back in that call, and no report follows
 *   aside       another conversation is opened in the window and talked to
 *   watch       the agent reads a new thread with more reads at once than
 *               one conversation may hold: some wait, the rest are refused,
 *               and a read after them is taken again
 *   window      the sidebar is hidden with ⌘B, another conversation is
 *               opened beside the main one with ⌘-click and asks the agent
 *               what the window shows: it is the focused pane on the right,
 *               the main conversation on the left, the sidebar closed; that
 *               pane is closed with ⌘W and the sidebar shown again, and the
 *               main conversation, asked in its composer, is the whole window
 *   long        another conversation takes `--long` turns of long streamed
 *               answers through the gateway; the last answer is whole, and
 *               still whole after its agent is closed and its history opened
 *               again, no slower for its length than `reopenGrowthMs`
 *               allows; the message sent in the window after them is answered
 *   fresh       a conversation is started in the window, from a new
 *               session's home, while `FRESH_BURST` others are started
 *               through the gateway at the same moment; its first message is
 *               sent and answered, and nothing the window or they sent is
 *               refused
 *   park        another conversation is left idle, unwatched, until the
 *               gateway parks its agent (its lease ends `idle`); it is opened
 *               in the window and the message sent there starts the agent
 *               again on its provider session
 *   rename      another conversation is renamed from its row's More menu;
 *               the gateway's list and the window say the new title, and it
 *               keeps it after a message sent there
 *   delete      another conversation is deleted from its row's More menu,
 *               once asked; the gateway refuses it as deleted, and neither
 *               list nor the window holds it
 *   rail        the side rail is turned on in Settings › Advanced ›
 *               Experimental (under Settings the window's controls stay at
 *               the window's edge) and opened: the controls are asked to
 *               stand beside it, in the workspace's corner, and the titlebar
 *               starts where they end; a message sent with the rail open is
 *               answered; closed, opened again, and turned off while open,
 *               they follow it back to the window's edge
 *   linking     native linking is turned on, then off, with the switch in
 *               Settings › Connections › Linked devices; each time the gateway
 *               saves config.json and starts itself again in the same
 *               process, the window comes back to the new state, and the
 *               message sent after it is answered
 *
 * Meanwhile `--parallel` background conversations take turns through the
 * gateway (every third one a watch), and `--followers` other clients follow
 * every conversation's events, the main view and the list
 * (`lib/stress-followers.mjs`), stopping and following again as they go, and
 * across every restart. At the end each follower must have applied every
 * conversation's events exactly once and in order. With `--parks` above 0 the
 * gateway parks every idle conversation nobody follows after
 * `PARK_IDLE_SECONDS`, so asides, threads and background conversations are
 * parked and started again as the run goes. At the end every conversation
 * must be at rest, every turn ended as
 * planned, and nothing denied automatically or hidden behind a display limit.
 *
 * Build the gateway first (`cargo build -p nessa-server`). How to add a step
 * is in CODING_STANDARDS.md, "Conversation stress".
 *
 *   pnpm test:e2e:stress                        # the pull request's run
 *   pnpm test:e2e:stress -- --rounds 60 --parallel 12 --seed 7
 *   NESSA_STRESS_ROUNDS=60 pnpm test:e2e:stress # the same, from the environment
 */
import catalog from "../../../crates/nessa-sdk/data/models.json" with { type: "json" }
import { randomUUID } from "node:crypto"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"

import { readWithReviews } from "../../../scripts/conversation-smoke/reviews.mjs"
import { turnEnded } from "../../../scripts/mcp-test-server/evidence.mjs"
import {
  SAYS,
  STRESS,
  WATCH_CALLS,
  stressScenario,
} from "../../../scripts/mcp-test-server/scenarios/stress.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { CannotRun, log } from "./lib/cli.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import {
  connectClient,
  panelTarget,
  startGatewayStack,
  waitFor,
} from "./lib/gateway-stack.mjs"
import { main } from "./lib/run.mjs"
import { writeView } from "./lib/scripted-evidence.mjs"
import {
  composerControls,
  css,
  keys,
  names,
  offeredLabel,
  selectorFor,
} from "./lib/selectors.mjs"
import { freePort } from "./lib/server.mjs"
import { Follower } from "./lib/stress-followers.mjs"
import {
  openStressConversation,
  selectedStressConversation,
} from "./lib/stress-conversation.mjs"
import { observeGatewayRestarts } from "./lib/stress-restart.mjs"
import { KNOBS, describe, knobsFrom, plan, random } from "./lib/stress-plan.mjs"

const INSTANCE = "conversation-stress"

const meta = {
  name: "conversation-stress",
  summary:
    "many rounds in one conversation through the gateway and the window, with access switches, stops, failures, restarts, threads and side conversations",
  defaults: { engine: "chromium", layout: "columns" },
  options: {
    agent: { type: "string", default: "claude" },
    evidence: { type: "string" },
    ...Object.fromEntries(Object.keys(KNOBS).map((name) => [name, { type: "string" }])),
  },
  help: `
Usage: node verification/desktop/scripts/conversation-stress.mjs [options]

Needs: the gateway built (cargo build -p nessa-server; or MCP_LIVE_NESSA). No
harness and no sign-in: the gateway runs scripted-agent.mjs with the stress
scenario (scripts/mcp-test-server/scenarios/stress.mjs). It starts its own
gateway; --url is not used.

Numbers (a flag, else NESSA_STRESS_<NAME>, else the default):
${Object.entries(KNOBS)
  .map(
    ([name, knob]) =>
      `  --${name.padEnd(9)} ${String(knob.default).padStart(3)}  ${knob.what}`,
  )
  .join("\n")}

Options:
  --agent claude|codex  the harness profile the scripted agent speaks (default:
                        claude; Linux offers full access only on Claude's model)
  --evidence <dir>      write the plan, acp.jsonl, mcp.jsonl, gateway.log and views/`,
}

/**
 * How long a conversation sits idle, unwatched, before the gateway parks its
 * agent in this run (`limits.agentIdleSeconds`). Short, so the run sees it;
 * the gateway looks four times within it.
 */
const PARK_IDLE_SECONDS = 3

/**
 * How many chunks each answer of the `long` step streams, and how many bytes
 * each carries. One answer stays within the 16 KB of text the gateway's view
 * shows of a message, which clips a longer one; the saved history keeps all
 * of every answer.
 */
const LONG_CHUNKS = 40
const LONG_CHUNK_BYTES = 300
/**
 * How many other conversations the `fresh` step creates and sends to at the
 * same moment as the window's own first message: more than the sixteen
 * command owners the gateway once allowed at a time, which refused the
 * seventeenth as `conversation_storage_unavailable` instead of queueing it.
 */
const FRESH_BURST = 24
/** The clients that burst is spread over, each well within one socket's 16 requests at once. */
const FRESH_CLIENTS = 3
/**
 * How much longer opening the `long` conversation again may take than
 * opening one of a single long turn, both closed first so their whole saved
 * history is replayed: half a second of noise, and a little for each turn.
 * Measured against the short one in the same run, so a loaded machine slows
 * both. On a 4-core machine, replayed in pages each turn adds about 15 ms;
 * replayed fact by fact (#1006) each added about 60 ms, past this.
 */
const reopenGrowthMs = (turns) => 500 + 20 * turns

const fold = (text) =>
  String(text ?? "")
    .replace(/\s+/g, " ")
    .trim()

/** Everything the run shares: the stack, the page, and the client in use now. */
class Run {
  constructor(stack, opened, knobs, restartConsole) {
    this.restartConsole = restartConsole
    this.stack = stack
    this.opened = opened
    this.page = opened.page
    this.knobs = knobs
    this.client = stack.client
    this.mode = "ask"
    /** Gateway restarts so far: a request in flight across one may lose its answer. */
    this.restarts = 0
    this.main = null
    /** Turns a background conversation has in flight; a restart waits for none. */
    this.busy = 0
    this.paused = null
    this.background = []
    /** Other conversations a step opened that the final rest check sweeps too. */
    this.checked = []
    /** Conversations a step deleted: followers let them go, and nothing sweeps them. */
    this.deleted = new Set()
    this.sent = new Map()
    /** Where native linking listens once the `linking` step turns it on. */
    this.nativeAddress = stack.nativeAddress
  }

  /** Waits while a restart holds the gateway. */
  async enter() {
    while (this.paused) await this.paused
    this.busy += 1
  }

  leave() {
    this.busy -= 1
  }

  /** Runs `body` with no background turn in flight and none starting. */
  async exclusive(body) {
    let release
    this.paused = new Promise((resolve) => (release = resolve))
    try {
      await waitFor(() => this.busy === 0, 120_000)
      return await body()
    } finally {
      this.paused = null
      release()
    }
  }
}

/** An error with its code and cause, which a client failure keeps apart from its message. */
const explain = (error) =>
  [error?.message, error?.code, error?.cause?.message ?? error?.cause?.code]
    .filter(Boolean)
    .join(" · ")

const dock = (page) => page.locator(css.conversationDock)

async function pageText(page) {
  return fold(
    await page
      .locator(css.workspace)
      .innerText()
      .catch(() => ""),
  )
}

/** Waits up to `ms` for the window to show `text`. */
const shows = (page, text, ms = 30_000) =>
  waitFor(async () => (await pageText(page)).includes(fold(text)), ms).then(Boolean)

/**
 * Reads until `ready` holds for the turn `at` names: a message index, or
 * `{ text }`, the turn the person sent as `text` (every text in a run is its
 * own). A read keeps only the latest turns and the agent's own turns can land
 * in between, so a sent turn is found by its text, never its position.
 */
async function untilTurn(run, conversationId, at, ready, seconds = 60) {
  const end = Date.now() + seconds * 1000
  let view
  let turn
  do {
    try {
      view = await readWithReviews(run.client, conversationId)
      turn =
        typeof at === "number"
          ? view.messages[at]
          : view.messages.find((each) => each.userText === at.text)
      if (turn && ready(turn, view)) return { view, turn }
    } catch (error) {
      // A read across a restart fails; the next one is on the new client.
      log(`read ${conversationId}: ${error.message}`)
    }
    await sleep(200)
  } while (Date.now() < end)
  return { view, turn }
}

/**
 * Reads `conversationId` until `ready` holds for its whole view, which may
 * have no turns: a conversation whose first submit was refused has none.
 */
async function untilView(run, conversationId, ready, seconds = 60) {
  const end = Date.now() + seconds * 1000
  let view
  do {
    try {
      view = await readWithReviews(run.client, conversationId)
      if (ready(view)) return { view }
    } catch (error) {
      log(`read ${conversationId}: ${error.message}`)
    }
    await sleep(200)
  } while (Date.now() < end)
  return { view }
}

const ended = (turn) => turnEnded(turn.status)

/**
 * Turns the side rail preview on or off where a person does: Settings ›
 * Advanced › Experimental. Leaves Settings, back in the workspace.
 */
async function setSideRail(page, on, failures) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await page.locator(css.settingsCategory, { hasText: names.advanced }).click()
  // A category with one tab draws no tab list.
  const tab = page.locator(css.settingsTab, { hasText: names.experimental })
  if (await tab.count()) await tab.click()
  const toggle = page.locator(`${selectorFor.settingRow("side-rail")} [role='switch']`)
  if ((await toggle.getAttribute("aria-checked")) !== String(on)) await toggle.click()
  const switched = await waitFor(
    async () => (await toggle.getAttribute("aria-checked")) === String(on),
    5_000,
  )
  if (!switched) failures.push(`the side rail's switch did not turn ${on ? "on" : "off"}`)
  // Settings has the window's corner: whatever the workspace under it draws,
  // the window's controls stand at the window's edge. The workspace answers a
  // turned-on rail after it draws it, so the asks are read once they stop.
  const drawn = await waitFor(
    async () => (await windowControls(page)).rail === (on ? "open" : "off"),
    10_000,
  )
  const now = await quietControls(page)
  if (!drawn || now.leads.at(-1) !== 0)
    failures.push(
      `under Settings the rail is ${now.rail} and the window's controls lead by ${now.leads.at(-1)}, not 0`,
    )
  await page.getByRole("button", { name: names.leaveSettings }).first().click()
  await page.waitForSelector(css.settings, { state: "detached", timeout: 10_000 })
  const shown = await waitFor(
    async () => (await page.locator(css.sideRailToggle).count()) === (on ? 1 : 0),
    10_000,
  )
  if (!shown) failures.push(`the side rail's toggle is ${on ? "missing" : "still shown"}`)
}

/** `windowControls` once the page has asked nothing new of the host for a second. */
async function quietControls(page) {
  let now = await windowControls(page)
  for (let quiet = 0, at = Date.now(); quiet < 4 && Date.now() - at < 10_000;) {
    await sleep(250)
    const next = await windowControls(page)
    quiet = next.leads.length === now.leads.length ? quiet + 1 : 0
    now = next
  }
  return now
}

/**
 * Waits until the rail is `rail` (open, closed, or off), the window's controls
 * were last asked to stand where the workspace starts, and the titlebar's
 * first control starts where they end. False, with the last measurement said,
 * when that never holds.
 */
async function controlsBeside(page, rail, failures) {
  const holds = (now) =>
    now.rail === rail &&
    [now.lead, now.inset, now.control].every(Number.isFinite) &&
    now.leads.at(-1) === now.lead &&
    Math.abs(now.control - (now.lead + now.inset)) <= 0.5
  if (await waitFor(async () => holds(await windowControls(page)), 10_000)) return true
  const now = await windowControls(page)
  failures.push(
    `rail ${rail}: the window's controls were asked to lead by ${JSON.stringify(now.leads)}, ` +
      `the workspace starts at ${now.lead}, its titlebar's first control at ${now.control} (inset ${now.inset})`,
  )
  return false
}

/**
 * Where the window's controls stand: every distance the page asked the host
 * to move them in (`__fakeHostLeads`), where the workspace starts, the
 * controls' inset from it, and where the titlebar's first control is.
 */
function windowControls(page) {
  return page.evaluate(
    ({ frame, workspace, control }) => {
      const at = document.querySelector(workspace)
      const first = document.querySelector(control)
      return {
        rail: document.querySelector(frame)?.dataset.rail,
        leads: [...(window.__fakeHostLeads ?? [])],
        lead: Math.round(at?.getBoundingClientRect().left ?? NaN),
        inset: parseFloat(
          getComputedStyle(at).getPropertyValue("--desktop-window-controls-inset"),
        ),
        control: Math.round(first?.getBoundingClientRect().left ?? NaN),
      }
    },
    {
      frame: css.workspaceWindow,
      workspace: css.workspace,
      control: css.titlebarSidebarToggle,
    },
  )
}

/** The row of the conversation `conversationId` in the window's list. */
const sessionRow = (page, conversationId) =>
  page.locator(selectorFor.sessionListRowFor(conversationId))

/**
 * Chooses `label` in the More menu of `conversationId`'s row: the pointer
 * over the row shows its buttons, as a person sees them.
 */
async function choose(page, conversationId, label) {
  const row = sessionRow(page, conversationId)
  await row.hover({ timeout: 15_000 })
  await row.locator('[data-session-action="more"]').click({ timeout: 10_000 })
  await page
    .getByRole("menuitem", { name: label, exact: true })
    .click({ timeout: 10_000 })
}

/** Waits for `conversationId`'s row to say `title`. */
async function rowShown(page, conversationId, title, failures) {
  const shown = await sessionRow(page, conversationId)
    .filter({ has: page.getByText(title, { exact: true }) })
    .waitFor({ timeout: 15_000 })
    .then(() => true)
    .catch(() => false)
  if (!shown) failures.push(`the window never listed ${JSON.stringify(title)}`)
}

/**
 * Types `text` into the open conversation's composer and presses Send.
 * Resolves with the turns the conversation already had, so a turn after it
 * is one not among them. A read keeps only the latest turns, so a turn is
 * never found by its position. A send the window refuses or keeps is a
 * failure, not a retry. `known`, the turns the caller already read, skips
 * that read, which opens the conversation's agent. `where` is the composer,
 * when more than one pane shows one.
 */
async function send(run, conversationId, text, failures, known, where = dock(run.page)) {
  const before = known ?? (await seenTurns(run, conversationId))
  const box = where.locator("textarea")
  await box.fill(text)
  const button = where.getByRole(...composerControls.send)
  await button.click({ timeout: 10_000 })
  const taken = await waitFor(async () => (await box.inputValue()) === "", 10_000)
  if (!taken) failures.push(`the composer kept ${JSON.stringify(text)} after Send`)
  run.sent.set(conversationId, (run.sent.get(conversationId) ?? 0) + 1)
  return before
}

/** The execution ids of the turns `conversationId` has now. */
async function seenTurns(run, conversationId) {
  const view = await run.client.conversation.read(conversationId)
  return new Set(view.messages.map((each) => each.executionId))
}

/**
 * The newest turn the gateway sent again on behalf of the one sent as `text`:
 * a message held behind a failure is stopped with it and sent again, as a
 * notice and then the original text.
 */
function carried(view, seen, text) {
  return view.messages.findLast(
    (each) =>
      !seen.has(each.executionId) &&
      each.userText !== text &&
      each.userText.endsWith(text),
  )
}

/**
 * The turn sent as `text`, once it ends, checked: `text` is in the
 * conversation exactly once, and its turn ended `status`. A turn stopped
 * behind a failure is followed to the turn that carries it on. `seen` are the
 * turns from before the send, left out of what a failure lists.
 */
async function settled(run, conversationId, seen, text, status, failures, seconds = 60) {
  const end = Date.now() + seconds * 1000
  let view
  let turn
  do {
    try {
      view = await readWithReviews(run.client, conversationId)
      turn = view.messages.find((each) => each.userText === text)
      if (turn?.status === "cancelled" && status !== "cancelled")
        turn = carried(view, seen, text) ?? turn
      if (turn && ended(turn) && !(turn.status === "cancelled" && status !== "cancelled"))
        break
    } catch (error) {
      log(`read ${conversationId}: ${error.message}`)
    }
    await sleep(200)
  } while (Date.now() < end)
  if (!turn) {
    failures.push(
      `${JSON.stringify(text)} is not in the conversation after ${seconds} s: ${JSON.stringify(
        view?.messages
          .filter((each) => !seen.has(each.executionId))
          .map((each) => [each.status, each.userText.slice(0, 60)]),
      )}`,
    )
    return { view }
  }
  const copies = view.messages.filter((each) => each.userText === text).length
  if (copies !== 1) failures.push(`${JSON.stringify(text)} was sent ${copies} times`)
  if (!ended(turn)) failures.push(`${JSON.stringify(text)} is still ${turn.status}`)
  else if (turn.status !== status)
    failures.push(
      `${JSON.stringify(text)} ended ${turn.status}${turn.error ? ` (${turn.error})` : ""}, not ${status}`,
    )
  return { view, turn }
}

async function expectShown(page, text, failures, ms) {
  if (!(await shows(page, text, ms)))
    failures.push(`the window does not show ${JSON.stringify(text)}`)
}

/** The visible agent card, once there is one; `null` after `ms`. */
async function card(page, ms = 30_000) {
  const cards = page.locator(css.agentApproval)
  const seen = await cards
    .first()
    .waitFor({ timeout: ms })
    .then(() => true)
    .catch(() => false)
  return seen ? cards : null
}

/** Answers the one card shown, by the effect `answer` names, from the review's own options. */
async function answerCard(run, conversationId, answer, failures, reason) {
  const cards = await card(run.page)
  if (!cards) {
    failures.push(
      `no approval card${reason ? ` for ${JSON.stringify(reason)}` : ""} within 30 s`,
    )
    return false
  }
  // The card just answered can still be leaving: the one to answer is the
  // card that asks `reason`, and the review is the one with that title.
  const asking = reason ? cards.filter({ hasText: reason }) : cards
  const showsIt = await asking
    .first()
    .waitFor({ timeout: 30_000 })
    .then(() => true)
    .catch(() => false)
  if (!showsIt) {
    failures.push(`the card does not ask ${JSON.stringify(reason)}`)
    return false
  }
  if (!(await waitFor(async () => (await cards.count()) === 1, 5_000)))
    failures.push(`${await cards.count()} approval cards shown, not 1`)
  const { view } = await untilTurn(
    run,
    conversationId,
    0,
    (_turn, current) =>
      current.permissions.some((each) => !reason || each.title === reason),
    10,
  )
  const permission =
    view?.permissions?.find((each) => !reason || each.title === reason) ??
    view?.permissions?.[0]
  const label = offeredLabel(permission?.options, answer === "allow" ? "allow" : "deny")
  if (!label) {
    failures.push(`the review offers no ${answer}`)
    return false
  }
  await asking
    .first()
    .getByRole("button", { name: label, exact: true })
    .click({ timeout: 10_000 })
  const gone = await waitFor(async () => {
    const current = await readWithReviews(run.client, conversationId)
    return !current.permissions.some((each) => each.review === permission.review)
  }, 30_000)
  if (!gone)
    failures.push(
      `the answer to ${JSON.stringify(reason ?? permission.title)} was not taken`,
    )
  return true
}

/** Picks `mode` in the open conversation's access shield. */
async function pickAccess(page, mode) {
  await dock(page).locator(composerControls.access).click({ timeout: 10_000 })
  await page.locator(composerControls.accessMenu).waitFor({ timeout: 10_000 })
  await page
    .locator(composerControls.accessMenu)
    .getByRole(...composerControls.accessItem[mode])
    .click({ timeout: 10_000 })
}

/** Waits for the gateway to say the conversation runs at `mode`. */
const accessIs = (run, conversationId, mode, ms = 20_000) =>
  waitFor(async () => {
    try {
      return (await run.client.conversation.read(conversationId)).approvalMode === mode
    } catch {
      return false
    }
  }, ms)

/** Waits until Send has turned into Stop. */
const stopShown = (page, ms = 15_000) =>
  dock(page)
    .getByRole(...composerControls.stop)
    .waitFor({ timeout: ms })
    .then(() => true)
    .catch(() => false)

const sendShown = (page, ms = 15_000) =>
  dock(page)
    .getByRole(...composerControls.send)
    .waitFor({ timeout: ms })
    .then(() => true)
    .catch(() => false)

/** A text no other turn of the run has: the kind's phrase, the step and the seed. */
const prompt = (run, phrase, step, extra = "") =>
  `${phrase} r${step.index}${extra} s${run.knobs.seed}`

/**
 * One of the `fresh` step's other conversations: created and sent to on
 * `client`, then followed on it to its end. Registered for the rest check
 * before it is created, so one whose create took effect and lost its answer
 * is still swept.
 */
async function startedBeside(run, client, step, n, failures) {
  const conversationId = randomUUID()
  const first = prompt(run, STRESS.reply, step, ` burst ${n}`)
  try {
    await client.conversation.create({ conversationId, agent: run.stack.agent })
  } catch (error) {
    failures.push(
      `a conversation started beside the window's was refused: ${explain(error)}`,
    )
    return
  }
  // Once it exists the rest check sweeps it, even if the submit's answer is
  // lost while its turn runs.
  run.checked.push(conversationId)
  try {
    await client.conversation.submit(client.conversation.prepare(conversationId, first))
  } catch (error) {
    failures.push(
      `a conversation started beside the window's was refused: ${explain(error)}`,
    )
    return
  }
  await settled(
    { ...run, client },
    conversationId,
    new Set(),
    first,
    "completed",
    failures,
  )
}

const steps = {
  async model(run, step, failures) {
    const picker = dock(run.page).locator(css.modelChip)
    if (await picker.isDisabled()) {
      failures.push("the established conversation's model picker is disabled")
      return
    }
    const before = await readWithReviews(run.client, run.main.id)
    const current = catalog.models.find(
      (model) => model.modelId === before.runtime?.model,
    )
    const target =
      current &&
      catalog.models.find(
        (model) =>
          model.provider === current.provider && model.modelId !== current.modelId,
      )
    if (!target)
      throw new CannotRun("no second catalog model in this conversation's provider")
    const text = prompt(run, STRESS.permission, step)
    const seen = await send(run, run.main.id, text, failures)
    if (!(await card(run.page))) {
      failures.push("no original review before model change")
      return
    }
    const waiting = await readWithReviews(run.client, run.main.id)
    const original = waiting.permissions.map((review) => ({
      review: review.review,
      executionId: review.executionId,
      options: review.options,
    }))
    const draft = prompt(run, STRESS.model, step)
    const box = dock(run.page).locator("textarea")
    await box.fill(draft)
    await picker.click()
    const photograph = run.evidence && !run.modelPhotographed
    if (photograph)
      await run.page.screenshot({
        path: join(run.evidence, `model-${run.engine}-picker.png`),
        scale: "css",
      })
    const options = run.page.locator(css.modelOption)
    await options.filter({ hasText: target.displayName }).first().click()
    const confirmed = await waitFor(async () => {
      const view = await readWithReviews(run.client, run.main.id)
      return view.runtime?.model === target.modelId && !view.modelChange ? view : null
    }, 15_000)
    if (!confirmed) {
      failures.push("the window's model choice was not confirmed by the gateway")
      return
    }
    if (
      !(await waitFor(
        async () =>
          (await picker.getAttribute("aria-label"))?.includes(target.displayName),
        5_000,
      ))
    )
      failures.push("the picker did not show the source-confirmed model")
    if (photograph) {
      await run.page.screenshot({
        path: join(run.evidence, `model-${run.engine}-confirmed-review-draft.png`),
        scale: "css",
      })
      run.modelPhotographed = true
    }
    const unchanged = confirmed.permissions.map((review) => ({
      review: review.review,
      executionId: review.executionId,
      options: review.options,
    }))
    if (JSON.stringify(original) !== JSON.stringify(unchanged))
      failures.push("model change replaced the original review or its choices")
    if ((await box.inputValue()) !== draft)
      failures.push("model change lost the unsent draft")
    if (
      !(await dock(run.page)
        .getByRole(...composerControls.stop)
        .count())
    )
      failures.push("model change removed Stop while its original turn waits")
    await answerCard(run, run.main.id, "allow", failures, "Report the rows")
    await settled(run, run.main.id, seen, text, "completed", failures)
    if ((await box.inputValue()) !== draft)
      failures.push("answering the original review lost the model-change draft")
    const next = await send(run, run.main.id, draft, failures)
    await settled(run, run.main.id, next, draft, "completed", failures)
    await expectShown(run.page, `${SAYS.model}${target.modelId}`, failures)
    if (run.evidence)
      writeFileSync(
        join(run.evidence, `model-${run.engine ?? "window"}-${step.index}.json`),
        JSON.stringify(
          {
            before: current.modelId,
            after: target.modelId,
            original,
            confirmed: confirmed.runtime,
            effect: confirmed.modelChangeEffect,
          },
          null,
          2,
        ),
      )
  },
  async reply(run, step, failures) {
    const text = prompt(run, STRESS.reply, step)
    const seen = await send(run, run.main.id, text, failures)
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    // A reply carried on behind a failure echoes its notice first.
    const echoed = turn && turn.userText !== text ? turn.userText.slice(0, 40) : text
    await expectShown(run.page, `${SAYS.echo}${echoed}`, failures)
    if ((await run.page.locator(css.sendFailure).count()) > 0)
      failures.push("the composer shows a send failure")
  },

  async permission(run, step, failures) {
    const text = prompt(run, STRESS.permission, step)
    const seen = await send(run, run.main.id, text, failures)
    if (step.mode === "full") {
      // Full access reaches the agent as its own mode: it asks for nothing.
      const { view } = await settled(run, run.main.id, seen, text, "completed", failures)
      if (view?.permissions?.length) failures.push("a review is open under full access")
      await expectShown(run.page, SAYS.allowed, failures)
      return
    }
    await answerCard(run, run.main.id, step.answer, failures, "Report the rows")
    await settled(run, run.main.id, seen, text, "completed", failures)
    await expectShown(
      run.page,
      step.answer === "allow" ? SAYS.allowed : SAYS.denied,
      failures,
    )
  },

  async reviews(run, step, failures) {
    const text = prompt(run, STRESS.reviews, step)
    const seen = await send(run, run.main.id, text, failures)
    const count = run.knobs.reviews
    for (let n = 1; n <= count && failures.length === 0; n += 1)
      await answerCard(run, run.main.id, "allow", failures, `Review ${n} of ${count}`)
    await settled(run, run.main.id, seen, text, "completed", failures, 120)
    await expectShown(run.page, SAYS.reviewed, failures)
  },

  async large(run, step, failures) {
    const text = prompt(run, STRESS.large, step)
    const seen = await send(run, run.main.id, text, failures)
    await answerCard(run, run.main.id, step.answer, failures, "A large review")
    await settled(run, run.main.id, seen, text, "completed", failures)
    await expectShown(
      run.page,
      step.answer === "allow" ? SAYS.allowed : SAYS.denied,
      failures,
    )
  },

  async stop(run, step, failures) {
    const text = prompt(run, STRESS.wait, step)
    const seen = await send(run, run.main.id, text, failures)
    await expectShown(run.page, SAYS.waiting, failures)
    if (!(await stopShown(run.page))) {
      failures.push("Send did not turn into Stop while the turn ran")
      return
    }
    await dock(run.page)
      .getByRole(...composerControls.stop)
      .click({ timeout: 10_000 })
    await settled(run, run.main.id, seen, text, "cancelled", failures)
    if (!(await sendShown(run.page))) failures.push("Stop did not turn back into Send")
  },

  async fail(run, step, failures) {
    const text = prompt(run, STRESS.fail, step)
    const seen = await send(run, run.main.id, text, failures)
    await settled(run, run.main.id, seen, text, "failed", failures)
    await expectShown(run.page, SAYS.beforeFailure, failures)
  },

  async switch(run, step, failures) {
    if (step.during) {
      const text = prompt(run, STRESS.pause, step)
      const seen = await send(run, run.main.id, text, failures)
      if (!(await stopShown(run.page)))
        failures.push("Send did not turn into Stop while the turn ran")
      await pickAccess(run.page, step.to)
      const held = await waitFor(
        async () =>
          fold(
            await dock(run.page)
              .locator(composerControls.notice)
              .innerText()
              .catch(() => ""),
          ).includes("Access changes when the agent stops"),
        10_000,
      )
      // The turn can end before the notice is read; only a running one owes it.
      const { turn } = await untilTurn(run, run.main.id, { text }, () => true, 5)
      if (!held && turn && !ended(turn))
        failures.push("the composer did not say the change waits for the turn")
      await settled(run, run.main.id, seen, text, "completed", failures)
    } else {
      await pickAccess(run.page, step.to)
    }
    if (!(await accessIs(run, run.main.id, step.to)))
      failures.push(`the gateway does not run the conversation at ${step.to}`)
    const shield = await dock(run.page)
      .locator(composerControls.access)
      .getAttribute("data-access-mode")
      .catch(() => null)
    if (shield !== composerControls.accessValue[step.to])
      failures.push(
        `the shield shows ${shield}, not ${composerControls.accessValue[step.to]}`,
      )
    run.mode = step.to
  },

  async restart(run, step, failures) {
    const text = prompt(run, STRESS.cut, step)
    await run.exclusive(async () => {
      const seen = await send(run, run.main.id, text, failures)
      await expectShown(run.page, SAYS.pausing, failures)
      // It may wait behind a turn of the agent's own, such as a thread's report.
      const { turn, view } = await untilTurn(
        run,
        run.main.id,
        { text },
        (current) => current.status === "running",
        60,
      )
      if (turn?.status !== "running") {
        failures.push(
          `the turn to cut off is ${turn?.status ?? "missing"}, not running: ${JSON.stringify(
            {
              messages: view?.messages
                .filter((each) => !seen.has(each.executionId))
                .map((each) => [each.status, each.userText.slice(0, 60)]),
              pending: view?.pending?.length,
            },
          )}`,
        )
        return
      }
      await run.restartConsole.during(run.opened, async () => {
        const before = failures.length
        const old = run.client
        run.restarts += 1
        await run.stack.gateway.restart(step.signal)
        old.close()
        run.client = await connectClient(run.stack.gateway, INSTANCE, "panel")
        // The cut-off turn is carried on by a successor: a notice, then the same text.
        const resumed = await waitFor(async () => {
          const view = await run.client.conversation.read(run.main.id)
          return view.messages.find(
            (each) =>
              !seen.has(each.executionId) &&
              each.userText !== text &&
              each.userText.endsWith(text) &&
              each.status === "completed",
          )
        }, 90_000)
        if (!resumed) {
          const view = await run.client.conversation.read(run.main.id).catch(() => null)
          failures.push(
            `the cut-off turn was not resumed after ${step.signal}: ${JSON.stringify(
              view?.messages
                .filter((each) => !seen.has(each.executionId))
                .map((each) => [each.status, each.userText.slice(0, 60)]),
            )}`,
          )
          return
        }
        if (!/restarted/i.test(resumed.userText))
          failures.push("the resumed turn does not say Nessa restarted")
        const resumedInWindow = await waitFor(async () => {
          const text = await pageText(run.page)
          return (
            text.includes(fold(resumed.userText)) && text.includes(fold(SAYS.resumed))
          )
        }, 60_000)
        if (!resumedInWindow)
          failures.push(
            "the window does not show the current resumed turn and its completion",
          )
        return failures.length === before
      })
    })
  },

  async linking(run, step, failures) {
    const gateway = run.stack.gateway
    const config = join(gateway.directory, "ci", "instances", INSTANCE, "config.json")
    await run.exclusive(async () => {
      const pid = gateway.server.pid
      if (!(await run.page.$(css.settings))) await run.page.keyboard.press(keys.settings)
      await run.page.locator(css.settingsCategory, { hasText: names.connections }).click()
      await run.page.locator(css.settingsTab, { hasText: names.linkedDevices }).click()
      for (const enabled of [true, false]) {
        const want = enabled ? "on" : "off"
        const before = await waitFor(
          async () =>
            (await run.page.locator(css.linked).getAttribute("data-linked")) ===
            (enabled ? "off" : "on"),
          30_000,
        )
        if (!before) {
          failures.push(`linking was not ${enabled ? "off" : "on"} before the switch`)
          return
        }
        await run.restartConsole.during(run.opened, async () => {
          const before = failures.length
          const old = run.client
          run.restarts += 1
          await run.page.locator(`${css.linked} [role='switch']`).click()
          const shown = await waitFor(
            async () =>
              (await run.page.locator(css.linked).getAttribute("data-linked")) === want,
            60_000,
          )
          if (!shown) failures.push(`the window did not come back with linking ${want}`)
          old.close()
          run.client = await connectClient(gateway, INSTANCE, "panel")
          const answered = await run.client.pairing.pending().then(
            () => "on",
            (error) =>
              error?.refusal === "pairing_not_configured" ? "off" : String(error),
          )
          if (answered !== want)
            failures.push(`the gateway answers linking ${answered}, not ${want}`)
          const saved = JSON.parse(readFileSync(config, "utf8")).native
          if (
            (saved?.enabled !== false) !== enabled ||
            saved?.listenAddress !== run.nativeAddress
          )
            failures.push(`config.json holds native ${JSON.stringify(saved)}`)
          if (gateway.server.pid !== pid || gateway.server.exitCode !== null)
            failures.push("the gateway did not start again in the same process")
          return failures.length === before
        })
      }
      // Esc does not close Settings; its back control returns to the app.
      await run.page.getByRole("button", { name: "Back to nessa Agent" }).first().click()
      await run.page.waitForSelector(css.settings, { state: "detached", timeout: 10_000 })
    })
    // The restarted gateway still answers the conversation, in the window.
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async rail(run, step, failures) {
    await run.exclusive(async () => {
      // Turned on, the rail is drawn under Settings at once; covered, the
      // workspace leaves the window's controls at the window's edge.
      await setSideRail(run.page, true, failures)
      if (failures.length) return
      const toggle = run.page.locator(css.sideRailToggle)
      if ((await toggle.getAttribute("aria-disabled")) === "true") {
        failures.push("the window had no room for the side rail")
        return
      }
      for (const open of [true, false, true]) {
        if ((await toggle.getAttribute("aria-expanded")) !== String(open))
          await toggle.click()
        if (!(await controlsBeside(run.page, open ? "open" : "closed", failures))) return
        // The workspace still takes a message with the rail beside it.
        if (open && failures.length === 0) await steps.reply(run, step, failures)
      }
      // Turned off with the rail open: the rail goes, and back from Settings
      // the controls stay at the window's edge rather than beside a rail.
      await setSideRail(run.page, false, failures)
      await controlsBeside(run.page, "off", failures)
    })
  },

  async delegate(run, step, failures) {
    const text = prompt(run, STRESS.delegate, step)
    const seen = await send(run, run.main.id, text, failures)
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    if (!turn) return
    await expectShown(run.page, SAYS.delegated, failures)
    // The thread's turn ends, and its report wakes this conversation.
    const report = await waitFor(async () => {
      const view = await run.client.conversation.read(run.main.id)
      return view.messages.find(
        (each) =>
          !seen.has(each.executionId) &&
          each.userText.startsWith(STRESS.report) &&
          ended(each),
      )
    }, 90_000)
    if (!report)
      failures.push("the thread's report never reached the conversation that sent it")
    else if (report.status !== "completed")
      failures.push(`the report's turn ended ${report.status}`)
    else await expectShown(run.page, SAYS.report, failures)
  },

  async await(run, step, failures) {
    const text = prompt(run, STRESS.await, step)
    const seen = await send(run, run.main.id, text, failures)
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    if (!turn) return
    const said = (turn.parts ?? []).map((part) => part.text ?? "").join("")
    const found = new RegExp(`${SAYS.awaited}([0-9a-f-]+): (.*)`, "s").exec(said)
    const reply = `${SAYS.echo}${STRESS.reply} awaited by the parent`
    if (!found) {
      failures.push(
        `the await turn did not say what it waited for: ${said.slice(0, 160)}`,
      )
      return
    }
    if (!found[2].startsWith(reply))
      failures.push(
        `one subscribe call did not return the thread's reply: ${found[2].slice(0, 160)}`,
      )
    await expectShown(run.page, `${SAYS.awaited}${found[1]}: ${reply}`, failures)
    // The reply came back in the call, so no report wakes the conversation.
    // A report would be queued as soon as the thread's turn settled, which
    // was before the call returned.
    await sleep(3_000)
    const view = await run.client.conversation.read(run.main.id)
    if (
      view.messages.some(
        (each) =>
          !seen.has(each.executionId) &&
          each.userText.startsWith(STRESS.report) &&
          each.userText.includes(found[1]),
      )
    )
      failures.push("a report followed the reply the agent already took with subscribe")
  },

  async watch(run, step, failures) {
    const text = prompt(run, STRESS.watch, step)
    const seen = await send(run, run.main.id, text, failures)
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    if (turn) watched(turn, failures)
  },

  async window(run, step, failures) {
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " beside")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    run.checked.push(conversationId)
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    await settled(run, conversationId, new Set(), first, "completed", failures)
    if (failures.length) return
    // ⌘B hides the sidebar, which makes room for a pane beside; ⌘-click on a
    // row in the session list then opens it there, focused. The page is a Mac's.
    await run.page.keyboard.press(keys.toggleSidebar)
    await run.page
      .locator(selectorFor.sessionListRowFor(conversationId), {
        hasText: await titleOf(run, conversationId),
      })
      .click({
        modifiers: ["Meta"],
        timeout: 15_000,
      })
    if (
      !(await waitFor(
        async () => (await run.page.locator(css.pane).count()) === 2,
        10_000,
      ))
    ) {
      failures.push(
        `opening beside left ${await run.page.locator(css.pane).count()} panes, not 2`,
      )
      return
    }
    // Asked in the composer of the pane beside, the focused one.
    const beside = await windowRead(
      run,
      conversationId,
      step,
      " beside",
      failures,
      (said) =>
        said.you?.position === "right" &&
        said.you.focused === true &&
        said.windows?.length === 1 &&
        said.windows[0].sidebar === "closed" &&
        said.windows[0].sessionList === "open" &&
        said.windows[0].panes.some(
          (pane) =>
            pane.threadId === run.main.id &&
            pane.position === "left" &&
            pane.focused === false,
        ),
    )
    if (beside) {
      const window = beside.windows[0]
      if (window.arrangement !== "columns" || window.showing !== "panes")
        failures.push(
          `the window reads as ${window.arrangement} showing ${window.showing}`,
        )
      const left = window.panes.find((pane) => pane.threadId === run.main.id)
      if (
        left?.you !== false ||
        left?.available === false ||
        left?.kind !== "conversation"
      )
        failures.push(`the main conversation's pane reads ${JSON.stringify(left)}`)
    }
    // ⌘W closes the focused pane: the one beside.
    await run.page.keyboard.press(keys.closePane)
    if (
      !(await waitFor(
        async () => (await run.page.locator(css.pane).count()) === 1,
        10_000,
      ))
    ) {
      failures.push("⌘W did not close the pane beside")
      return
    }
    await run.page.keyboard.press(keys.toggleSidebar)
    // The main conversation is the pane left. Not opened from its row: the
    // row beside it quotes the window, the main conversation's title with it.
    // Asked from the main conversation's composer: it is the whole window now.
    await windowRead(
      run,
      run.main.id,
      step,
      " alone",
      failures,
      (said) =>
        said.you?.position === "full" &&
        said.you.focused === true &&
        said.windows?.length === 1 &&
        said.windows[0].panes.length === 1,
    )
  },

  async aside(run, step, failures) {
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " aside")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    await settled(run, conversationId, new Set(), first, "completed", failures)
    const title = await titleOf(run, conversationId)
    await openStressConversation(run.page, conversationId, title)
    const second = prompt(run, STRESS.reply, step, " aside again")
    const seen = await send(run, conversationId, second, failures)
    await settled(run, conversationId, seen, second, "completed", failures)
    await expectShown(run.page, `${SAYS.echo}${second}`, failures)
    await openStressConversation(run.page, run.main.id, run.main.title)
  },

  async fresh(run, step, failures) {
    // A person starts a conversation the way the window does: a new
    // session's home, its first message typed and sent. The window creates
    // the conversation and sends that message itself, with no client library
    // in between. Meanwhile `FRESH_BURST` other conversations are created and
    // sent to at once, so the window's create and send arrive while many
    // other commands are still in flight: a gateway that refuses a command
    // for the commands around it, rather than queueing it, fails here.
    const text = prompt(run, STRESS.reply, step, ` fresh ${run.engine}`)
    // Spread over clients of their own: one socket takes only so many
    // requests at once, and what this step asks about is the gateway's whole.
    // Each conversation is also read on its own client, for the same reason.
    const clients = []
    const burst = []
    try {
      for (let n = 0; n < FRESH_CLIENTS; n += 1)
        clients.push(await connectClient(run.stack.gateway, INSTANCE, "panel"))
      for (let n = 0; n < FRESH_BURST; n += 1)
        burst.push(startedBeside(run, clients[n % FRESH_CLIENTS], step, n, failures))
      await run.page.keyboard.press(keys.newSession)
      const home = run.page.locator(`${css.focusedPane} ${css.paneHome}`)
      await home.locator(css.field).waitFor({ timeout: 15_000 })
      await home.locator(css.field).fill(text)
      await home.getByRole(...composerControls.send).click({ timeout: 10_000 })
      const conversationId = await waitFor(
        () => selectedStressConversation(run.page, text),
        30_000,
      )
      if (!conversationId) {
        failures.push("the conversation started in the window was never listed")
      } else {
        run.checked.push(conversationId)
        await settled(run, conversationId, new Set(), text, "completed", failures)
        await expectShown(run.page, `${SAYS.echo}${text}`, failures)
      }
      // Answered or not, a send the window took and then failed says so in
      // its composer, with the code it was refused with.
      const refused = run.page.locator(css.sendFailure)
      if ((await refused.count()) > 0)
        failures.push(
          `the first message of a new conversation failed: ${fold(await refused.first().innerText())}`,
        )
    } finally {
      // A step that threw leaves no burst running into the next one.
      await Promise.allSettled(burst)
      for (const client of clients) client.close()
    }
    await openStressConversation(run.page, run.main.id, run.main.title)
  },

  async park(run, step, failures) {
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " park")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    const { view: before } = await settled(
      run,
      conversationId,
      new Set(),
      first,
      "completed",
      failures,
    )
    if (failures.length) return
    // Nothing reads it meanwhile: a read opens its agent, a follower keeps it.
    await sleep((PARK_IDLE_SECONDS + 4) * 1000)
    const parked = await followedOnce(run, conversationId)
    const lease = parked.lease
    if (lease?.state !== "ended" || lease?.cause !== "idle") {
      failures.push(
        `the idle conversation was not parked: its lease is ${lease?.state ?? "missing"}${
          lease?.cause ? ` (${lease.cause})` : ""
        }`,
      )
      return
    }
    // A person opens the parked conversation and writes in it: the window
    // shows it with no agent running, and Send starts one.
    await openStressConversation(
      run.page,
      conversationId,
      await titleOf(run, conversationId),
    )
    const second = prompt(run, STRESS.reply, step, " after parking")
    const seen = await send(
      run,
      conversationId,
      second,
      failures,
      new Set(before.messages.map((each) => each.executionId)),
    )
    const { view: after } = await settled(
      run,
      conversationId,
      seen,
      second,
      "completed",
      failures,
    )
    await expectShown(run.page, `${SAYS.echo}${second}`, failures)
    if (after?.lease?.state !== "live")
      failures.push(`the next message left the lease ${after?.lease?.state ?? "missing"}`)
    else if (!(after.lease.revision > (before.lease?.revision ?? 0)))
      failures.push("the next message ran on the parked agent's lease, not a new one")
    await openStressConversation(run.page, run.main.id, run.main.title)
  },

  async rename(run, step, failures) {
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " rename")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    run.checked.push(conversationId)
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    const { view: before } = await settled(
      run,
      conversationId,
      new Set(),
      first,
      "completed",
      failures,
    )
    if (failures.length) return
    const renamed = `Renamed s${run.knobs.seed} ${step.index}`
    // As a person does: the row's More menu under the pointer, Rename, the
    // new title typed over the old one, and Return.
    await choose(run.page, conversationId, "Rename")
    const field = run.page.locator("[data-session-rename]")
    await field.fill(renamed)
    await field.press("Enter")
    const listed = await waitFor(async () => {
      const { conversations } = await run.client.conversation.list({})
      return (
        conversations.find((each) => each.conversationId === conversationId)?.title ===
        renamed
      )
    }, 15_000)
    if (!listed) failures.push("the gateway's list never said the new title")
    await rowShown(run.page, conversationId, renamed, failures)
    // A message after the rename keeps the person's title, not the first message's.
    await openStressConversation(run.page, conversationId, renamed)
    const second = prompt(run, STRESS.reply, step, " after rename")
    const seen = await send(
      run,
      conversationId,
      second,
      failures,
      new Set(before.messages.map((each) => each.executionId)),
    )
    await settled(run, conversationId, seen, second, "completed", failures)
    if ((await titleOf(run, conversationId)) !== renamed)
      failures.push("a message after the rename changed the title back")
    await openStressConversation(run.page, run.main.id, run.main.title)
  },

  async delete(run, step, failures) {
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " delete")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    await settled(run, conversationId, new Set(), first, "completed", failures)
    if (failures.length) return
    await choose(run.page, conversationId, "Delete…")
    const asked = run.page.locator("[data-delete-session]")
    await asked.waitFor({ timeout: 10_000 })
    // Marked as the delete is confirmed, and no sooner: from here a
    // follower's stream of it ending is the delete's, and before it any
    // failure of that stream is a failure.
    run.deleted.add(conversationId)
    await asked.locator('[data-delete-action="confirm"]').click({ timeout: 10_000 })
    const gone = await waitFor(async () => {
      try {
        await run.client.conversation.read(conversationId)
        return false
      } catch (error) {
        return error?.code === "conversation_deleted"
      }
    }, 60_000)
    if (!gone) failures.push("the gateway still reads the deleted conversation")
    const { conversations } = await run.client.conversation.list({})
    if (conversations.some((each) => each.conversationId === conversationId))
      failures.push("the gateway's list still holds the deleted conversation")
    const hidden = await waitFor(
      async () => (await sessionRow(run.page, conversationId).count()) === 0,
      15_000,
    )
    if (!hidden) failures.push("the window still lists the deleted conversation")
  },

  async long(run, step, failures) {
    const conversationId = randomUUID()
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    run.checked.push(conversationId)
    // Each answer streams LONG_CHUNKS chunks, so the history grows by about
    // 12 KB a turn and every save comes after a long one.
    const took = []
    let text
    for (let turn = 0; turn < step.turns; turn += 1) {
      text = prompt(run, STRESS.long, step, ` t${turn}`)
      const started = Date.now()
      await run.client.conversation.submit(
        run.client.conversation.prepare(conversationId, text),
      )
      const before = failures.length
      const { turn: done } = await settled(
        run,
        conversationId,
        new Set(),
        text,
        "completed",
        failures,
      )
      took.push(Date.now() - started)
      if (!done || failures.length > before) return
    }
    log(
      `long conversation: ${step.turns} turns, first ${took[0]} ms, last ${took.at(-1)} ms`,
    )
    // The newest answer is whole: every chunk, in order, once.
    const view = await run.client.conversation.read(conversationId)
    const answer = view.messages.find((each) => each.userText === text)
    const said = (answer?.parts ?? []).map((part) => part.text ?? "").join("")
    const chunks = [
      ...said.matchAll(new RegExp(` (\\d+) x{${LONG_CHUNK_BYTES}}`, "g")),
    ].map((match) => Number(match[1]))
    if (
      !said.startsWith(`${SAYS.long}${text}`) ||
      chunks.length !== LONG_CHUNKS ||
      chunks.some((index, at) => index !== at)
    )
      failures.push(
        `the last long answer came back with ${chunks.length} of ${LONG_CHUNKS} chunks: ${JSON.stringify(said.slice(0, 80))}`,
      )
    // Its agent is closed, so the next opening replays the whole saved
    // history: that takes no longer for its length than the budget allows,
    // and the answer read back after it is still whole.
    const short = randomUUID()
    await run.client.conversation.create({
      conversationId: short,
      agent: run.stack.agent,
    })
    run.checked.push(short)
    const one = prompt(run, STRESS.long, step, " alone")
    await run.client.conversation.submit(run.client.conversation.prepare(short, one))
    await settled(run, short, new Set(), one, "completed", failures)
    const alone = await reopened(run, short)
    const again = await reopened(run, conversationId)
    log(`long conversation reopened in ${again.ms} ms; one turn in ${alone.ms} ms`)
    const growth = reopenGrowthMs(step.turns)
    if (again.ms - alone.ms > growth)
      failures.push(
        `reopening ${step.turns} turns took ${again.ms} ms, one turn ${alone.ms} ms: over ${growth} ms more`,
      )
    if (again.view.transcriptState !== "complete")
      failures.push(
        `the reopened long conversation was read ${again.reads} times and is still ${again.view.transcriptState}`,
      )
    else {
      const kept = again.view.messages.find((each) => each.userText === text)
      if ((kept?.parts ?? []).map((part) => part.text ?? "").join("") !== said)
        failures.push(
          "the last long answer changed when the conversation was opened again",
        )
    }
    // A person opens the long conversation and writes in it.
    await openStressConversation(
      run.page,
      conversationId,
      await titleOf(run, conversationId),
    )
    const last = prompt(run, STRESS.reply, step, " after the long answers")
    const seen = await send(run, conversationId, last, failures)
    await settled(run, conversationId, seen, last, "completed", failures)
    await expectShown(run.page, `${SAYS.echo}${last}`, failures)
    await openStressConversation(run.page, run.main.id, run.main.title)
  },
}

/**
 * Asks `conversationId`'s agent what the window shows, until what it says
 * meets `holds`, asked in the focused pane's composer. The window tells the
 * gateway what it shows on its own request, so the turn can start a moment
 * before the gateway has the change. Resolves with the answer that met
 * `holds`, else `null`; the window shows each answer.
 */
async function windowRead(run, conversationId, step, extra, failures, holds) {
  let said
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    const text = prompt(run, STRESS.window, step, `${extra} a${attempt}`)
    // The pane asking is the focused one: its composer is the one typed in.
    const seen = await send(
      run,
      conversationId,
      text,
      failures,
      undefined,
      run.page.locator(css.focusedPane).locator(css.conversationDock),
    )
    const { turn } = await settled(run, conversationId, seen, text, "completed", failures)
    if (!turn) return null
    const reply = (turn.parts ?? []).map((part) => part.text ?? "").join("")
    const at = reply.indexOf(SAYS.window)
    try {
      said = JSON.parse(reply.slice(at + SAYS.window.length))
    } catch {
      failures.push(`the window turn did not say what it read: ${reply.slice(0, 120)}`)
      return null
    }
    await expectShown(run.page, `${SAYS.window}{`, failures)
    if (holds(said)) return said
    await sleep(500)
  }
  failures.push(`${extra.trim()}, the agent read the window as ${JSON.stringify(said)}`)
  return null
}

/**
 * Close `conversationId`'s agent, then open it again with a read, timed;
 * the view is read on to the head, as one read takes a bounded number of
 * records.
 */
async function reopened(run, conversationId) {
  await run.client.conversation.close(conversationId)
  const started = Date.now()
  let view = await run.client.conversation.read(conversationId)
  const ms = Date.now() - started
  let reads = 1
  for (; view.transcriptState !== "complete" && reads < 20; reads += 1) {
    await sleep(100)
    view = await run.client.conversation.read(conversationId)
  }
  return { ms, view, reads }
}

/**
 * The first view frame of `conversationId` from a view subscription, which
 * never opens an agent. Closed at once: a follower keeps its agent from
 * being parked.
 */
async function followedOnce(run, conversationId) {
  let seen
  let lost
  const first = new Promise((resolve, reject) => ((seen = resolve), (lost = reject)))
  const opening = run.client.subscriptions.view(conversationId, {
    view: (each) => seen(each.view),
    ended: (end) => lost(new Error(`the view ended ${end.reason}`)),
  })
  try {
    return await Promise.race([
      first,
      opening.then(() => first),
      sleep(15_000).then(() => {
        throw new Error("no view frame in 15 s")
      }),
    ])
  } finally {
    // Closed whatever happened, so nothing follows it afterwards; an open
    // that failed left nothing to close.
    const subscription = await opening.catch(() => null)
    await subscription?.close().catch(() => {})
  }
}

/**
 * What a watch turn's reads came to, as the agent said it: of `WATCH_CALLS`
 * at once, some were refused for want of room and the rest waited, and
 * nothing else happened to any of them.
 */
function watched(turn, failures) {
  const said = (turn.parts ?? []).map((part) => part.text ?? "").join("")
  const at = said.indexOf(SAYS.watched)
  let results
  try {
    results = JSON.parse(said.slice(at + SAYS.watched.length))
  } catch {
    failures.push(
      `the watch turn did not say what its reads came to: ${said.slice(0, 120)}`,
    )
    return
  }
  if (!Array.isArray(results) || results.length !== WATCH_CALLS) {
    failures.push(`the watch turn made ${results?.length} reads, not ${WATCH_CALLS}`)
    return
  }
  const refused = results.filter((each) => each?.error)
  const waited = results.filter((each) => !each?.error)
  // The agent's own server takes only so many calls at once, and of those
  // the conversation only so many waiting reads: both refusals are expected.
  const other = refused.filter(
    (each) => !["subscription_capacity", "call_refused"].includes(each.error),
  )
  if (other.length)
    failures.push(`reads were refused ${[...new Set(other.map((each) => each.error))]}`)
  if (!refused.some((each) => each.error === "subscription_capacity"))
    failures.push(`no read of ${WATCH_CALLS} at once was refused for want of room`)
  if (!waited.length) failures.push("no read at all was taken")
  if (waited.some((each) => each.timedOut !== true && !each.events?.length))
    failures.push("a read that was taken neither waited nor returned events")
}

/**
 * One background conversation: `turns` turns through the gateway, a reply
 * then a permission it allows, over and over. Waits while a restart holds
 * the gateway. Resolves with what went wrong.
 */
async function background(run, n) {
  const failures = []
  const conversationId = randomUUID()
  try {
    await run.enter()
    try {
      await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    } finally {
      run.leave()
    }
    for (let turn = 0; turn < run.knobs.turns; turn += 1) {
      const permission = turn % 3 === 1
      const watch = turn % 3 === 2
      const phrase = watch ? STRESS.watch : permission ? STRESS.permission : STRESS.reply
      const text = `${phrase} b${n}.${turn} s${run.knobs.seed}`
      await run.enter()
      try {
        await run.client.conversation.submit(
          run.client.conversation.prepare(conversationId, text),
        )
        const { permissionTargets } = await import("@nessa/client")
        const answered = new Set()
        const { turn: done } = await untilTurn(
          run,
          conversationId,
          { text },
          (current, view) => {
            const waiting = view.permissions[0]
            if (waiting && !answered.has(waiting.review)) {
              answered.add(waiting.review)
              const option = waiting.options.find((each) => each.effect === "allow")
              const restarts = run.restarts
              void run.client.conversation
                .answer(permissionTargets(view)[0], option.id)
                .catch((error) => {
                  // The socket's control lanes were full, so nothing was
                  // applied; the code says to send it again, as a client must.
                  if (error?.code === "temporarily_unavailable") {
                    answered.delete(waiting.review)
                    return
                  }
                  // A restart can drop the answer's acknowledgement, not the
                  // answer: the turn must still complete, checked below.
                  if (run.restarts === restarts)
                    failures.push(
                      `background ${n} at ${new Date().toISOString()}: answer: ${explain(error)}`,
                    )
                })
            }
            return ended(current)
          },
          120,
        )
        if (done?.status !== "completed")
          failures.push(
            `background ${n} turn ${turn} ended ${done?.status ?? "missing"}${done?.error ? ` (${done.error})` : ""}`,
          )
        else if (watch) watched(done, failures)
      } finally {
        run.leave()
      }
    }
  } catch (error) {
    failures.push(`background ${n} at ${new Date().toISOString()}: ${explain(error)}`)
  }
  return { conversationId, failures }
}

/** What must hold once everything has run, in every conversation the run made. */
async function titleOf(run, conversationId) {
  const title = await waitFor(async () => {
    const { conversations } = await run.client.conversation.list({})
    return conversations.find((each) => each.conversationId === conversationId)?.title
  }, 15_000)
  if (!title)
    throw new CannotRun(`conversation ${conversationId} has no title to find it by`)
  return title
}

async function atRest(run, ids) {
  const failures = []
  for (const conversationId of ids) {
    const { view } = await untilView(
      run,
      conversationId,
      (current) => current.messages.every(ended) && current.pending.length === 0,
      60,
    )
    if (!view) {
      failures.push(`${conversationId} could not be read`)
      continue
    }
    const running = view.messages.filter((each) => !ended(each))
    if (running.length)
      failures.push(`${conversationId} still has ${running.length} turns at work`)
    for (const turn of view.messages) {
      const said = `${turn.error ?? ""} ${(turn.parts ?? []).map((part) => part.text ?? "").join(" ")}`
      if (/Denied automatically|No human answered/i.test(said))
        failures.push(`${conversationId}: a review was denied automatically`)
      if (/display limit/i.test(said))
        failures.push(`${conversationId}: a review was hidden`)
    }
    if (view.permissions.length) failures.push(`${conversationId} still has open reviews`)
  }
  const text = await pageText(run.page)
  if (/display limit|Denied automatically/i.test(text))
    failures.push("the window says a review was hidden or denied automatically")
  return failures
}

async function startStack(options) {
  const knobs = knobsFrom(options)
  const nativeAddress = `127.0.0.1:${await freePort()}`
  const directory = mkdtempSync(join(tmpdir(), "nessa-stress-"))
  const scenario = join(directory, "stress.json")
  writeFileSync(
    scenario,
    JSON.stringify(
      stressScenario({
        reviews: knobs.reviews,
        longChunks: LONG_CHUNKS,
        longChunkBytes: LONG_CHUNK_BYTES,
      }),
    ),
  )
  if (options.evidence) {
    mkdirSync(options.evidence, { recursive: true })
    writeFileSync(
      join(options.evidence, "plan.txt"),
      `${JSON.stringify(knobs)}\n${describe(plan(knobs))}\n`,
    )
  }
  let stack
  try {
    stack = await startGatewayStack(options, INSTANCE, {
      as: "panel",
      scenario,
      evidence: options.evidence,
      limits: { agentIdleSeconds: knobs.parks > 0 ? PARK_IDLE_SECONDS : 0 },
      // Off, at a port of its own: the `linking` step turns it on from Settings.
      native: { enabled: false, listenAddress: nativeAddress },
    })
  } catch (error) {
    rmSync(directory, { recursive: true, force: true })
    throw error
  }
  const close = stack.close
  stack.close = async () => {
    try {
      await close()
    } finally {
      rmSync(directory, { recursive: true, force: true })
    }
  }
  return panelTarget(stack, () => ({
    knobs,
    agent: options.agent,
    nativeAddress,
    endpoint: stack.gateway.url.replace(/^http/, "ws"),
  }))
}

await main(
  meta,
  async ({ options, rep, target: stack }) => {
    const knobs = stack.knobs
    const planned = plan(knobs)
    log(`conversation-stress ${JSON.stringify(knobs)}\n${describe(planned)}`)
    const origin = new URL(stack.url).origin
    await withEngines(options, rep, async (engine, browser) => {
      const layout = options.layouts[0]
      let restartConsole
      const opened = await openPage(browser, {
        preparePage: async (page) => {
          restartConsole = observeGatewayRestarts(page, stack.endpoint)
        },
        url: `${origin}/desktop.html`,
        layout,
        lines: { engine, layout },
        initScripts: [
          [gatewayHost, { endpoint: stack.endpoint, credential: stack.credential }],
        ],
      })
      const run = new Run(stack, opened, knobs, restartConsole)
      run.evidence = options.evidence
      run.engine = engine
      const base = { engine, layout }
      let followers = []
      try {
        const id = randomUUID()
        const first = `${STRESS.reply} main s${knobs.seed} ${engine}`
        const setup = []
        await run.client.conversation.create({ conversationId: id, agent: stack.agent })
        await run.client.conversation.submit(run.client.conversation.prepare(id, first))
        await settled(run, id, new Set(), first, "completed", setup)
        run.main = { id, title: await titleOf(run, id) }
        await openStressConversation(run.page, run.main.id, run.main.title)
        if (setup.length) {
          rep.add({ name: "setup", ...base, failures: setup })
          return
        }
        run.background = Array.from({ length: knobs.parallel }, (_, n) =>
          background(run, n),
        )
        const { subscriptionLimits } = await import("@nessa/client")
        followers = Array.from(
          { length: knobs.followers },
          (_, n) =>
            new Follower({
              n,
              connect: () => connectClient(stack.gateway, INSTANCE, "panel"),
              limits: subscriptionLimits,
              random: random(knobs.seed * 1_000 + n + 1),
              mainId: run.main.id,
              deleted: run.deleted,
            }),
        )
        await Promise.all(followers.map((each) => each.start()))
        let stopped = null
        for (const step of planned) {
          const name = `${String(step.index).padStart(3, "0")} ${step.kind}`
          if (stopped) {
            rep.add({ name, ...base, cannotRun: true, error: `not run: ${stopped}` })
            continue
          }
          const at = Date.now()
          const failures = []
          let error
          try {
            await steps[step.kind](run, step, failures)
          } catch (thrown) {
            error = thrown
          }
          const entry = rep.add({
            name,
            ...base,
            ms: Date.now() - at,
            seen: step,
            failures,
            ...(error
              ? { error: explain(error), cannotRun: error instanceof CannotRun }
              : {}),
          })
          if (!entry.ok) {
            stopped = `${name} did not hold`
            if (options.evidence) {
              const view = await readWithReviews(run.client, run.main.id).catch(
                () => null,
              )
              if (view)
                writeView(
                  options.evidence,
                  `views/${engine}-${name.replace(" ", "-")}.json`,
                  view,
                )
              await run.page
                .screenshot({
                  path: join(options.evidence, `${engine}-${name.replace(" ", "-")}.png`),
                })
                .catch(() => {})
            }
          }
        }
        const done = await Promise.all(run.background)
        rep.add({
          name: "background",
          ...base,
          seen: { conversations: done.length, turns: knobs.turns },
          failures: done.flatMap((each) => each.failures),
        })
        rep.add({
          name: "at rest",
          ...base,
          failures: await atRest(run, [
            run.main.id,
            ...run.checked,
            ...done.map((each) => each.conversationId),
          ]),
        })
        await Promise.all(followers.map((each) => each.settle()))
        const followed = []
        for (const each of followers) followed.push(...(await each.check(run.client)))
        rep.add({
          name: "followers",
          ...base,
          seen: followers.map((each) => ({
            conversations: each.events.size,
            ...each.counts,
          })),
          failures: followed,
        })
        if (options.evidence) {
          const view = await readWithReviews(run.client, run.main.id).catch(() => null)
          if (view) writeView(options.evidence, `views/${engine}-main.json`, view)
        }
      } finally {
        await Promise.allSettled(run.background)
        await Promise.allSettled(followers.map((each) => each.close()))
        stack.client = run.client
        try {
          await opened.close()
        } finally {
          restartConsole.dispose()
        }
      }
    })
  },
  startStack,
)
