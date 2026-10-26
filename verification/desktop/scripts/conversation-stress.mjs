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
 *   usageLimit  the turn ends in a usage limit; the card shows, the project
 *               preference is turned on and back off, and the turn is not resumed
 *   switch      the access shield picks another mode, sometimes mid-turn,
 *               where the change waits for the turn to end
 *   restart     the gateway is killed (crash) or stopped (shutdown) mid-turn
 *               and started again; the turn is resumed and the window follows
 *   delegate    the agent creates a thread and messages it; the thread's
 *               report comes back as a turn of its own
 *   await       the agent messages a thread and waits for it with one
 *               subscribe call naming the sent turn: the thread's reply comes
 *               back in that call, and no report follows
 *   webhook     one granted conversation advances to a fixed source Head after
 *               a browser turn; independent native rows prove zero attempts
 *               under the unchanged loopback destination refusal policy
 *   aside       another conversation is opened in the window and talked to
 *   watch       the agent reads a new thread with more reads at once than
 *               one conversation may hold: some wait, the rest are refused,
 *               and a read after them is taken again
 *   window      the sidebar is hidden with ⌘B, another conversation is
 *               opened beside the main one with ⌘-click and asks the agent
 *               what the window shows: it is the focused pane on the right,
 *               the main conversation on the left, the sidebar closed; that
 *               pane is closed with ⌘W and the sidebar shown again, and the
 *               main conversation, asked in its composer, is the whole window;
 *               beside, the panes are regions of one flat chat surface that
 *               reaches the window's top, right and bottom edges: no pane
 *               draws a fill or rim of its own, and a hairline down the
 *               middle of the gutter between them divides them; no tab
 *               strip is drawn for the chats, each pane has one header in
 *               the titlebar's row, clear of the window's controls, and its
 *               picture runs up to the window's top edge — with the session
 *               list shown and with the panes alone
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
 *               Experimental and opened: the window's traffic lights stay
 *               where the host puts them above a compact rail; the top bar
 *               keeps its controls clear of them in every rail state; a message
 *               sent with the rail open is answered; closed, opened again,
 *               and turned off while open, the titlebar's controls go back to
 *               starting where the lights end
 *   onboarding  setup is replayed from Settings › Advanced › Setup: the host
 *               is asked once; in the setup window that ask opens, nothing
 *               dims the desktop, and a glow fills the screen and forms the
 *               window (glow, fill, form, open, in that order) before any of
 *               the window's words can be seen; welcome has no Back button
 *               and Close sits in the top-right corner; past it the picker
 *               has Back in the top-left corner, which returns to welcome;
 *               it lists Claude, Codex and a Local AI that is coming, never
 *               OpenCode, with each one's figure on the picture beside it;
 *               pointing at a row leaves the picture alone and clicking it
 *               turns the picture to that agent; Close then leaves setup,
 *               handing over as left, not finished; a second setup window,
 *               where Codex is not installed but can be downloaded, shows
 *               Codex as a download card with no row to pick, and no list
 *               of installed agents or Check downloads; its Download, pressed
 *               from the keyboard, fetches with the focus kept on the card,
 *               installs it, and the card becomes Codex's row, which takes
 *               the focus; setup is then finished
 *               on a ready agent: the window flies into the menu bar, asking
 *               the host where its icon is, and hands over finished on that
 *               agent; the message sent in the window after it is answered
 *   notice      a review waits in another conversation, which the window does
 *               not show: the host's system notice names its tool and the
 *               conversation's title, opening it brings that conversation
 *               up with its card, and answering there takes the notice back;
 *               a second review there stays up across a reload of the
 *               window, and is taken back once another client answers it
 *   linking     native linking is turned on, then off, with the switch in
 *               Settings › Connections › Linked devices; each time the gateway
 *               saves config.json and starts itself again in the same
 *               process, the window comes back to the new state, and the
 *               message sent after it is answered. While it is on, the
 *               gateway is found by a DNS-SD browse on its port, and Pair a
 *               device opens a code with the window's own sign-in, in a
 *               modal with one signal orb; Escape hides it, Show code
 *               brings the same code back, Cancel closes it, and leaving
 *               the tab and coming back still shows linking on
 *   drift       the gateway is stopped and started again with its staged
 *               runtime directory moved and a different agent PATH and
 *               TMPDIR, the way an app update stages a new tree and
 *               re-registers the launchd job; the next message sent in the
 *               window to the existing conversation is answered
 *   pool        native linking is turned on in Settings and two real
 *               `nessa env dial` workers pair with the gateway; a
 *               conversation created on `peer:any` is bound to one of them
 *               and stays there while it serves; that worker is stopped with
 *               SIGTERM, drains and exits, and the next message sent in the
 *               window binds the conversation to the other, with new provider
 *               context, as the window's heading and the binding both say;
 *               both workers stop and linking is turned off again
 *   launch      the window is reloaded while the host is still starting the
 *               gateway: the host holds the window's read, the startup
 *               screen never covers it, and once the gateway is up the
 *               window lists the conversation and the message sent after
 *               it is answered
 *   github      GitHub is connected in Settings › Connections › Accounts
 *               against a fake GitHub on this machine: the row shows the
 *               code GitHub handed out and where to enter it, and once the
 *               fake approves it shows the account's picture (GitHub's
 *               avatar host answered by the page), its login and
 *               "Connected", its actions out only under the pointer or
 *               keyboard focus; the sign-in is a private
 *               file the gateway never logs; an agent started after it has
 *               GH_TOKEN and a git credential helper for github.com, both the
 *               token, and so do the commands it runs through Nessa's own
 *               shell tool; Disconnect forgets it, and an agent started after
 *               that has neither, nor do its commands; the audit holds each
 *               step and no token
 *   upgrade     the gateway is stopped, its conversation metadata turned
 *               back into the file an older build wrote (version 6, or 7
 *               when a newer table holds rows), and started again: it
 *               upgrades the file to the current version, lists every
 *               conversation it listed before, and the message sent in the
 *               window after it is answered
 *   channel     a channel is made from the sidebar's +, asked in the
 *               transcript: named, a folder chosen in the host's picker that
 *               agents may not work in (the root) refused at its question,
 *               then one made for it; its model kept, notes typed, and a file
 *               attached from the host's picker; Create opens a new
 *               session's home in it, whose first message runs in the made
 *               folder with the notes in the agent's system prompt (Claude).
 *               A second channel works in a folder chosen in the picker, and
 *               its first message runs there; the gateway lists both, with
 *               their folders, and each conversation in its own
 *   trace       the agent makes three calls, one failing, then answers: the
 *               gateway's view keeps each call's start and end time and the
 *               input it was made with; the window draws them as one folded
 *               line, "Ran 3 tools", that opens to a row per call, each with
 *               its status and a bar on the shared time axis (at least as
 *               wide as it is tall, ending inside the axis), and a call's
 *               card shows its input
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
import { createHash, randomUUID } from "node:crypto"
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { basename, dirname, isAbsolute, join } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"

import { readWithReviews } from "../../../scripts/conversation-smoke/reviews.mjs"
import { MODELS, repoRoot } from "../../../scripts/mcp-test-server/local-gateway.mjs"
import { turnEnded } from "../../../scripts/mcp-test-server/evidence.mjs"
import {
  SAYS,
  STRESS,
  TABLE_LINES,
  TRACE_CALLS,
  TRACE_PAUSE_MS,
  WATCH_CALLS,
  stressScenario,
} from "../../../scripts/mcp-test-server/scenarios/stress.mjs"
import { browse } from "./lib/mdns-probe.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { CannotRun, log } from "./lib/cli.mjs"
import { gatewayHost, STARTING } from "./lib/fake-host.mjs"
import {
  connectClient,
  panelTarget,
  scriptedLaunch,
  startGatewayStack,
  webhookNoAttempt,
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
import {
  FAKE_ACCOUNT,
  FAKE_AVATAR,
  GITHUB_AVATARS,
  startFakeGithub,
} from "./lib/fake-github.mjs"
import { freePort } from "./lib/server.mjs"
import { Follower } from "./lib/stress-followers.mjs"
import { asOlderBuild, metadataFile, schemaVersion } from "./lib/stress-upgrade.mjs"
import {
  openStressConversation,
  selectedStressConversation,
} from "./lib/stress-conversation.mjs"
import { observeGatewayRestarts } from "./lib/stress-restart.mjs"
import { KNOBS, describe, knobsFrom, plan, random } from "./lib/stress-plan.mjs"
import { flatSurfaceFailures } from "./lib/workspace.mjs"
import { startWorker } from "./lib/stress-workers.mjs"

const INSTANCE = "conversation-stress"
/** The conversation metadata version this build writes (`schema.sql`'s last line). */
const CURRENT_METADATA_VERSION = Number(
  /PRAGMA user_version = (\d+);\s*$/.exec(
    readFileSync(
      new URL(
        "../../../crates/nessa-server/src/conversation/infrastructure/schema.sql",
        import.meta.url,
      ),
      "utf8",
    ),
  )[1],
)
/**
 * The environment that places a conversation on any of the gateway's paired
 * workers (`POOL` in crates/nessa-server/src/conversation/application/placement.rs).
 */
const POOL = "peer:any"

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
 * The `launch` step's host stays "starting" this long after the window is
 * reloaded, well under the host's hold (`STARTING.waitMs`). A refused window
 * covers itself with the startup screen at once, which is what the step
 * checks; `LAUNCH_LISTED_MS` only bounds how long listing may take once the
 * gateway is up.
 */
const LAUNCH_STARTING_MS = 3_000
const LAUNCH_LISTED_MS = 10_000
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
    /** What config.json names for it: every interface, on the same port. */
    this.nativeListen = stack.nativeListen
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
  const drawn = await waitFor(
    async () => (await railLayout(page)).rail === (on ? "open" : "off"),
    10_000,
  )
  if (!drawn) {
    const now = await railLayout(page)
    failures.push(
      on && now.rail === "closed"
        ? "the window had no room for the side rail"
        : `under Settings the rail is ${now.rail}, not ${on ? "open" : "off"}`,
    )
  }
  await page.getByRole("button", { name: names.leaveSettings }).first().click()
  await page.waitForSelector(css.settings, { state: "detached", timeout: 10_000 })
  const shown = await waitFor(
    async () => (await page.locator(css.sideRailToggle).count()) === (on ? 1 : 0),
    10_000,
  )
  if (!shown) failures.push(`the side rail's toggle is ${on ? "missing" : "still shown"}`)
}

/**
 * The expanded right panel is the chat in front's: its own tabs, or with
 * nothing open its note, no tab strip for the chats and no toggles for the
 * columns it covers. ⌘N docks it before the new session opens, so the
 * session is in front, in sight — never under the panel. The chat is
 * brought back, docked.
 */
async function expandedForItsChat(run, failures) {
  const page = run.page
  if (
    await page.locator('[role="tablist"]:not(#workspace-panel [role="tablist"])').count()
  )
    failures.push("the window draws a tab strip for the chats")
  const panel = page.locator("#workspace-panel")
  if (
    !(await panel.locator("[data-panel-empty], .workspace-panel-tabs").count()) ||
    ((await panel.locator("[data-panel-empty]").count()) &&
      (await panel.innerText()).trim() !== "Nothing open")
  )
    failures.push(
      `the expanded panel shows neither its tabs nor its note: "${await panel.innerText()}"`,
    )
  if (await page.locator(`${css.titlebar} [aria-controls="workspace-sidebar"]`).count())
    failures.push("the expanded panel's titlebar offers the sidebar it covers")
  await page.keyboard.press(keys.newSession)
  const docked = await waitFor(
    async () => (await page.locator("#workspace-panel[data-maximized]").count()) === 0,
    2000,
  )
  if (!docked) failures.push("⌘N left the panel expanded over the new session")
  const home = page.locator(`${css.focusedPane} ${css.paneHome}`)
  await home.locator(css.field).waitFor({ timeout: 15_000 })
  if (await home.evaluate((node) => node.closest("[inert]") !== null))
    failures.push("the new session opened under the panel")
  await openStressConversation(page, run.main.id, run.main.title)
}

/**
 * "nessa Studio" belongs to whatever column holds the bottom-left corner, at
 * any width: with the sidebar folded the session list still holds it and the
 * words stay; only with the panes alone at the window's edge are they gone.
 * Leaves the columns as it found them.
 */
async function studioWithTheCorner(page, failures) {
  const shown = () =>
    page.evaluate(
      () =>
        getComputedStyle(document.querySelector(".side-rail-studio") ?? document.body)
          .visibility === "visible",
    )
  const column = (controls) =>
    page.locator(`${css.titlebar} button[aria-controls="${controls}"]`)
  const list = page.locator(
    ["Hide", "Show"]
      .map((verb) => `${css.titlebar} button[aria-label^="${verb} Session List"]`)
      .join(", "),
  )
  const sidebar = column("workspace-sidebar")
  // Hidden, it fades first and turns invisible at the fade's end: a "shown"
  // read too soon after a change would see the fade. Settle, then read.
  const expect = async (want, what) => {
    await page.waitForTimeout(600)
    if (!(await waitFor(async () => (await shown()) === want, 2000)))
      failures.push(`"nessa Studio" is ${want ? "missing" : "shown"} ${what}`)
  }
  const open = async (button) =>
    (await button.count()) > 0 && (await button.getAttribute("aria-expanded")) === "true"
  const set = async (button, want) => {
    if ((await button.count()) > 0 && (await open(button)) !== want) await button.click()
  }
  // Away from the window's left edge, where the pointer would peek the sidebar out.
  const size = page.viewportSize()
  if (size) await page.mouse.move(size.width / 2, size.height / 2)
  const was = { sidebar: await open(sidebar), list: await open(list) }
  await set(sidebar, true)
  await expect(true, "beside the open sidebar")
  await set(sidebar, false)
  if ((await list.count()) > 0) {
    await set(list, true)
    await expect(true, "with the session list holding the corner")
    await set(list, false)
  }
  await expect(false, "over the panes alone")
  await set(list, was.list)
  await set(sidebar, was.sidebar)
}

/** The macOS traffic lights end at x=76 (`traffic_lights.rs`). */
const LIGHTS_END = 76

/** The compact rail and the window bar keep separate geometry. */
async function aroundTheLights(page, rail, failures) {
  const near = (a, b) => Math.abs(a - b) <= 0.5
  const holds = (now) =>
    now.rail === rail &&
    [now.at.workspace, now.at.inset, now.at.control].every(Number.isFinite) &&
    near(now.at.control, now.at.inset) &&
    now.at.control >= LIGHTS_END &&
    (rail === "open"
      ? near(now.at.workspace, now.at.railWidth) &&
        near(now.at.railTop, now.at.barBottom) &&
        near(now.at.railWidth, 64) &&
        near(now.at.item, now.at.railWidth / 2)
      : near(now.at.workspace, 0)) &&
    // The toggle is nessa's mark: in the footer's mark slot, right before
    // "nessa Studio", wherever the footer is drawn; at the corner otherwise.
    (rail === "off" ||
      (now.at.slot && now.at.toggleMiddle
        ? near(now.at.toggleMiddle[0], now.at.slot[0]) &&
          near(now.at.toggleMiddle[1], now.at.slot[1])
        : near(now.at.toggle, now.at.itemColumn)))
  if (await waitFor(async () => holds(await railLayout(page)), 10_000)) return true
  const now = await railLayout(page)
  failures.push(
    `rail ${rail} (now ${now.rail}) below window bar: ` + JSON.stringify(now.at),
  )
  return false
}

/**
 * The rail's state and, from the window's left edge: its width, the middle of
 * its first item and of its toggle, where the workspace and its first card start,
 * the window controls' inset from it, and where its titlebar's first control
 * starts.
 */
function railLayout(page) {
  return page.evaluate(
    ({ frame, workspace, control, rail, item, toggle, titlebar }) => {
      const box = (selector) => document.querySelector(selector)?.getBoundingClientRect()
      const middle = (selector) => {
        const at = box(selector)
        return at ? at.left + at.width / 2 : NaN
      }
      // A length token may be a calc(): resolved by laying it out.
      const length = (inside, token) => {
        const probe = document.createElement("div")
        probe.style.cssText = `position:absolute;visibility:hidden;width:var(${token})`
        inside.append(probe)
        const width = probe.getBoundingClientRect().width
        probe.remove()
        return width
      }
      // The workspace's first column: the rounded box at its left edge (the
      // docked sidebar, drawn flat but still cornered for its peek).
      const card = (inside) => {
        const left = inside.getBoundingClientRect().left
        const x = left + length(inside, "--desktop-gutter") + 2
        const panel = document
          .elementsFromPoint(x, innerHeight / 2)
          .find(
            (e) =>
              inside.contains(e) &&
              parseFloat(getComputedStyle(e).borderTopLeftRadius) >= 8 &&
              e.getBoundingClientRect().height > innerHeight / 2,
          )
        return panel?.getBoundingClientRect().left ?? NaN
      }
      const at = document.querySelector(workspace)
      return {
        rail: document.querySelector(frame)?.dataset.rail,
        at: {
          railWidth: box(rail)?.width ?? NaN,
          item: middle(item),
          toggle: box(toggle)?.left ?? NaN,
          toggleBottom: box(toggle)?.bottom ?? NaN,
          toggleMiddle: box(toggle)
            ? [
                box(toggle).left + box(toggle).width / 2,
                box(toggle).top + box(toggle).height / 2,
              ]
            : null,
          // The footer's mark slot the toggle stands in: the visible one, if any.
          slot: (() => {
            const found = [
              ...document.querySelectorAll(".workspace-identity .desktop-identity-mark"),
            ]
              .filter(
                (e) =>
                  getComputedStyle(e).visibility !== "hidden" && !e.closest("[inert]"),
              )
              .map((e) => e.getBoundingClientRect())
              .find((r) => r.width > 0 && r.left >= 0 && r.left < innerWidth)
            return found
              ? [found.left + found.width / 2, found.top + found.height / 2]
              : null
          })(),
          windowBottom: innerHeight,
          itemColumn: length(document.querySelector(frame), "--rail-column"),
          railTop: box(rail)
            ? box(rail).top +
              parseFloat(getComputedStyle(document.querySelector(rail)).paddingTop)
            : NaN,
          barBottom: box(titlebar)?.bottom ?? NaN,
          workspace: at.getBoundingClientRect().left,
          card: card(at),
          inset: length(document.querySelector(frame), "--desktop-window-controls-inset"),
          control: box(control)?.left ?? NaN,
        },
      }
    },
    {
      frame: css.workspaceWindow,
      workspace: css.workspace,
      control: `${css.titlebar} button`,
      titlebar: css.titlebar,
      rail: css.sideRail,
      item: `${css.sideRail} [data-rail-item]`,
      toggle: css.sideRailToggle,
    },
  )
}

/**
 * Toggles the rail from the page and follows "nessa Studio" every frame: it
 * stands in the corner beside the toggle, 8px past the toggle's mark, and
 * neither moves, not even for a frame. Settled, the toggle (with the rail
 * closed) stands on Agents' icon column. Returns what it found wrong.
 */
function footerThroughToggle(page) {
  return page.evaluate(async (toggleSelector) => {
    const words = () =>
      document.querySelector(".side-rail-studio .desktop-identity-words")
    const toggle = document.querySelector(toggleSelector)
    const mark = () => toggle.querySelector("svg").getBoundingClientRect()
    const middle = (e) => {
      const at = e.getBoundingClientRect()
      return at.left + at.width / 2
    }
    const failures = []
    if (!words()) failures.push(`no "nessa Studio" beside the rail's toggle`)
    const seen = []
    const started = performance.now()
    toggle.click()
    await new Promise((done) => {
      const frame = () => {
        if (words()) seen.push(words().getBoundingClientRect().left - mark().right)
        if (performance.now() - started < 900) requestAnimationFrame(frame)
        else done()
      }
      requestAnimationFrame(frame)
    })
    const off = seen.find((gap) => Math.abs(gap - 8) > 0.5)
    if (off !== undefined)
      failures.push(
        `"nessa Studio" stood ${off.toFixed(1)}px past the toggle's mark, not 8px`,
      )
    const icon = document.querySelector(".agents-overview-entry svg")
    if (toggle.getAttribute("aria-expanded") === "false" && icon) {
      const apart = middle(toggle) - middle(icon)
      if (Math.abs(apart) > 0.5)
        failures.push(
          `the closed rail's toggle is ${apart.toFixed(1)}px off Agents' icon`,
        )
    }
    return failures
  }, css.sideRailToggle)
}

/** The row of the conversation `conversationId` in the window's list. */
const sessionRow = (page, conversationId) =>
  page.locator(selectorFor.sessionListRowFor(conversationId))

/**
 * Chooses `label` in the More menu of `conversationId`'s row: the pointer
 * over the row shows its buttons, as a person sees them. The list re-sorts
 * as other conversations change, which can move the row out from under the
 * pointer and hide the button again; a person points at the row again, and
 * so does this, until the button takes the click.
 */
async function choose(page, conversationId, label) {
  const row = sessionRow(page, conversationId)
  const more = row.locator('[data-session-action="more"]')
  const end = Date.now() + 20_000
  for (;;) {
    await row.hover({ timeout: 15_000 })
    try {
      await more.click({ timeout: 2_000 })
      break
    } catch (error) {
      if (Date.now() > end) throw error
    }
  }
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

/**
 * Points the selected agent's command and its absolute file arguments at a
 * new directory. A script becomes a wrapper there that runs the real file:
 * a symlink would make `process.argv[1]` differ from the path the script
 * records, so it would never start, and a copy would break a root it
 * derives from its own path. The node binary and every other file are
 * symlinks, so the bytes stay where they are.
 */
function moveStagedRuntime(directory) {
  const configPath = join(directory, "ci", "instances", INSTANCE, "config.json")
  const config = JSON.parse(readFileSync(configPath, "utf8"))
  const runtime = config.agents.runtimes[config.agents.selected]
  const staged = mkdtempSync(join(tmpdir(), "nessa-stress-runtime-"))
  let n = 0
  const wrapScript = (path) => {
    const dest = join(staged, `${n++}-${basename(path)}`)
    writeFileSync(
      dest,
      `import { spawnSync } from "node:child_process"
import { realpathSync } from "node:fs"
const result = spawnSync(realpathSync(process.execPath), [${JSON.stringify(path)}, ...process.argv.slice(2)], { stdio: "inherit" })
process.exit(result.status ?? 1)
`,
    )
    return dest
  }
  const relocate = (path) => {
    if (typeof path !== "string" || !isAbsolute(path) || !existsSync(path)) return path
    if (lstatSync(path).isDirectory()) return path
    if (/\.(?:mjs|cjs|js)$/.test(path)) return wrapScript(path)
    const dest = join(staged, `${n++}-${basename(path)}`)
    symlinkSync(path, dest)
    return dest
  }
  runtime.command = relocate(runtime.command)
  // Only the staged runtime moves. An absolute argument such as
  // `--config /profiles/staging.toml` is the restoration identity.
  runtime.stagedRuntime = (runtime.stagedRuntime ?? []).map(relocate)
  writeFileSync(configPath, JSON.stringify(config))
  chmodSync(configPath, 0o600)
}

/**
 * Whether `turn`, the first after a handoff, was the one given the earlier
 * turns, on `model`: the scenario answers that turn alone with `SAYS.carried`
 * and the prompt it was given, which quotes `earlier`, a message said before.
 */
function carriedBy(turn, model, what, earlier, failures) {
  const said = (turn?.parts ?? [])
    .filter((part) => part.kind === "text")
    .map((part) => part.text)
    .join("")
  if (!said.startsWith(`${SAYS.carried}${model}: `))
    failures.push(
      `the first turn after ${what} said ${JSON.stringify(said.slice(0, 200))}, not that it carried the earlier turns on ${model}`,
    )
  else if (!said.includes(JSON.stringify(earlier)))
    failures.push(`the first turn after ${what} was not given ${JSON.stringify(earlier)}`)
}

/**
 * Whether the window shows, whole, the reply on `model` to `message`: it says
 * back the prompt it answered, which ends with `message`, so no other reply
 * quotes it, and it is shown whole once that end is.
 */
async function carriedShown(page, model, message) {
  return page
    .locator('.workspace-message[data-role="agent"]:visible')
    .filter({ hasText: `${SAYS.carried}${model}` })
    .filter({ hasText: message })
    .first()
    .waitFor({ timeout: 15_000 })
    .then(() => true)
    .catch(() => false)
}

/** The scripted agent a `handoff` step switches to: the one the run does not start on. */
function otherAgent(agent) {
  return agent === "claude" ? "codex" : "claude"
}

/** Picks `model` in a composer's picker: its provider's tab, then the model. */
async function pickModel(page, picker, model) {
  await picker.click()
  await page
    .locator(css.modelProviderTab)
    .filter({ hasText: providerName(model.provider) })
    .first()
    .click()
  await page
    .locator(css.modelOption)
    .filter({ hasText: model.displayName })
    .first()
    .click()
}

/** A catalog provider's tab in the picker, as the window names it (`composer-options.ts`). */
function providerName(provider) {
  return (
    { anthropic: "Anthropic", openai: "OpenAI", opencode: "OpenCode" }[provider] ??
    provider
  )
}

/**
 * Makes a channel with the sidebar's + as a person does, answering each
 * question as it is asked, and answers the gateway's listing of it; `null`
 * when it was not made. With `refusedFolder`, that folder is chosen first and
 * Create must be refused at the folder question, before a folder made for it
 * is taken instead.
 */
async function makeChannel(
  run,
  failures,
  { name, folder, refusedFolder, notes, resource },
) {
  const { page } = run
  const pick = (kind, value) =>
    page.evaluate(
      ([kind, value]) => window.__fakeHostPicks[kind].push(value),
      [kind, value],
    )
  const asking = (stepName) =>
    page
      .locator(`${css.newChannelAsked}[data-step="${stepName}"]`)
      .waitFor({ timeout: 10_000 })
      .then(() => true)
      .catch(() => {
        failures.push(`the new channel flow never asked its ${stepName}`)
        return false
      })
  // With evidence asked for, each question is photographed as it is asked.
  const photograph = (label) =>
    run.evidence
      ? page
          .screenshot({ path: join(run.evidence, `channel-${run.engine}-${label}.png`) })
          .catch(() => {})
      : Promise.resolve()
  const head = page.locator(`${css.sidebar} .workspace-section-head`).first()
  await head.hover()
  await head.locator(css.channelAdd).click({ timeout: 10_000 })
  const flow = page.locator(css.newChannel)
  if (!(await asking("name"))) return null
  await page.locator(`${css.newChannelField}[data-step="name"]`).fill(name)
  await photograph(`${name}-1-name`)
  await page.keyboard.press(keys.enter)
  if (!(await asking("folder"))) return null
  const folderChoice = refusedFolder ?? folder
  if (folderChoice) {
    await pick("folders", folderChoice)
    await flow.getByRole("button", { name: "Choose a folder…" }).click()
    await flow
      .getByRole("button", { name: "Use this folder" })
      .waitFor({ timeout: 5_000 })
    await photograph(`${name}-2-folder`)
    await flow.getByRole("button", { name: "Use this folder" }).click()
  } else {
    await flow.getByRole("button", { name: "Make one for me" }).click()
  }
  if (!(await asking("model"))) return null
  await photograph(`${name}-3-model`)
  await page.keyboard.press(keys.enter)
  if (!(await asking("notes"))) return null
  if (notes) await page.locator(`${css.newChannelField}[data-step="notes"]`).fill(notes)
  await photograph(`${name}-4-notes`)
  await page.keyboard.press(keys.enter)
  if (!(await asking("resources"))) return null
  if (resource) {
    await pick("resources", [resource])
    await flow.getByRole("button", { name: "Add files…" }).click()
    const row = flow.locator(css.newChannelResource, { hasText: basename(resource) })
    if (
      !(await row.waitFor({ timeout: 5_000 }).then(
        () => true,
        () => false,
      ))
    )
      failures.push(`the attached ${basename(resource)} is not listed in the flow`)
  }
  await photograph(`${name}-5-resources`)
  const create = flow.getByRole("button", { name: /^Create channel/ })
  await create.click()
  if (refusedFolder) {
    if (!(await asking("folder"))) return null
    const said = await flow
      .locator(`${css.newChannelAsked}[data-step="folder"]`)
      .getByText("Agents can’t work there", { exact: false })
      .waitFor({ timeout: 10_000 })
      .then(
        () => true,
        () => false,
      )
    if (!said)
      failures.push(`choosing ${refusedFolder} was not refused at the folder question`)
    await photograph(`${name}-6-refused`)
    await flow.getByRole("button", { name: "Make one for me" }).click()
    await create.click()
  }
  const gone = await flow.waitFor({ state: "detached", timeout: 15_000 }).then(
    () => true,
    () => false,
  )
  if (!gone) {
    failures.push(
      `the channel ${name} was not made: ${fold(await flow.innerText().catch(() => ""))}`,
    )
    await page.keyboard.press(keys.escape)
    return null
  }
  const { channels } = await run.client.channels.list()
  const listed = channels.find((channel) => channel.name === name)
  if (!listed) failures.push(`the gateway does not list the channel ${name}`)
  return listed ?? null
}

/**
 * The first message of a new session in `channel`, from the home Create left
 * open: it is listed in that channel, and its agent works in the channel's
 * folder, told the channel's notes (Claude's system prompt, where it has one).
 */
async function talkInChannel(run, step, channel, failures, notes) {
  const text = prompt(run, STRESS.channel, step, ` ${channel.name}`)
  const home = run.page.locator(`${css.focusedPane} ${css.paneHome}`)
  await home.locator(css.field).waitFor({ timeout: 15_000 })
  await home.locator(css.field).fill(text)
  await home.getByRole(...composerControls.send).click({ timeout: 10_000 })
  const conversationId = await waitFor(
    () => selectedStressConversation(run.page, text),
    30_000,
  )
  if (!conversationId) {
    failures.push(`the first message in ${channel.name} was never listed`)
    return
  }
  run.checked.push(conversationId)
  const { view } = await settled(
    run,
    conversationId,
    new Set(),
    text,
    "completed",
    failures,
  )
  const { conversations } = await run.client.conversation.list({})
  const summary = conversations.find((each) => each.conversationId === conversationId)
  if (summary?.channelId !== channel.channelId)
    failures.push(
      `the conversation started in ${channel.name} is in ${summary?.channelId ?? "no channel"}`,
    )
  const reply = (view?.messages.find((each) => each.userText === text)?.parts ?? [])
    .filter((part) => part.kind === "text")
    .map((part) => part.text)
    .join("")
  if (!reply.includes(`${SAYS.channel}${channel.folder}\n`))
    failures.push(
      `the agent in ${channel.name} did not work in ${channel.folder}: ${fold(reply.slice(0, 300))}`,
    )
  if (notes && run.stack.agent === "claude" && !reply.includes(notes))
    failures.push(`the agent in ${channel.name} was not told the channel's notes`)
}

const steps = {
  async model(run, step, failures) {
    const picker = dock(run.page).locator(css.modelChip)
    // A mode switch just before this step can still be settling. The picker
    // is disabled only while that change is in flight.
    const enabled = await waitFor(async () => !(await picker.isDisabled()), 10_000)
    if (!enabled) {
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
    // Picking only stages the model: the picker shows it, and the gateway is
    // asked for nothing until the next message is sent.
    if (
      !(await waitFor(
        async () =>
          (await picker.getAttribute("aria-label"))?.includes(target.displayName),
        5_000,
      ))
    )
      failures.push("the picker did not show the staged model")
    const asked = await waitFor(async () => {
      const view = await readWithReviews(run.client, run.main.id)
      return view.runtime?.model !== current.modelId || view.modelChange ? view : null
    }, 2_000)
    if (asked) {
      failures.push("picking a model changed the conversation before a message was sent")
      return
    }
    const staged = await readWithReviews(run.client, run.main.id)
    if (photograph) {
      await run.page.screenshot({
        path: join(run.evidence, `model-${run.engine}-staged-review-draft.png`),
        scale: "css",
      })
      run.modelPhotographed = true
    }
    const unchanged = staged.permissions.map((review) => ({
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
    // Sending applies the staged model first, and the message runs on it.
    const next = await send(run, run.main.id, draft, failures)
    const confirmed = await waitFor(async () => {
      const view = await readWithReviews(run.client, run.main.id)
      return view.runtime?.model === target.modelId && !view.modelChange ? view : null
    }, 15_000)
    if (!confirmed) {
      failures.push("sending did not apply the staged model")
      return
    }
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
    const text = prompt(run, STRESS.reply, step, step.extra ?? "")
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

  async listAnswer(run, step, failures) {
    const text = prompt(run, STRESS.permission, step)
    const seen = await send(run, run.main.id, text, failures)
    // The conversation asks; the session list's Needs you card asks the same.
    const listCard = run.page.locator(
      `[data-list-group="needs-you"] .workspace-request-card:has([data-session-row="${run.main.id}"])`,
    )
    const shown = await listCard
      .locator(".workspace-request")
      .first()
      .waitFor({ timeout: 30_000 })
      .then(() => true)
      .catch(() => false)
    if (!shown) {
      failures.push("the session list shows no request card for the waiting conversation")
      return
    }
    const words = async (locator) =>
      ((await locator.first().textContent()) ?? "").replace(/\s+/g, " ").trim()
    const fromList = await words(listCard.locator(".workspace-approval-command"))
    const fromPane = await words(
      run.page.locator(`${css.agentApproval} .workspace-approval-command`),
    )
    if (fromList !== fromPane)
      failures.push(
        `the list's card reads "${fromList}", the conversation's "${fromPane}"`,
      )
    await listCard
      .locator(`button[data-answer="${step.answer === "allow" ? "once" : "deny"}"]`)
      .first()
      .click()
    await settled(run, run.main.id, seen, text, "completed", failures)
    await expectShown(
      run.page,
      step.answer === "allow" ? SAYS.allowed : SAYS.denied,
      failures,
    )
    // Answered, the card goes with the row out of Needs you.
    if (!(await waitFor(async () => (await listCard.count()) === 0, 10_000)))
      failures.push("the answered card is still in Needs you")
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

  async usageLimit(run, step, failures) {
    const text = prompt(run, STRESS.limit, step)
    const seen = await send(run, run.main.id, text, failures)
    const settledTurn = await settled(run, run.main.id, seen, text, "failed", failures)
    if (!settledTurn.turn) return
    const limit = settledTurn.turn.usageLimit
    if (!limit || limit.provider !== "claude" || limit.recovery !== "unknown")
      failures.push(
        `the turn is not an unknown Claude usage limit: ${JSON.stringify(limit)}`,
      )
    if (settledTurn.turn.error)
      failures.push(`a usage limit kept a diagnostic: ${settledTurn.turn.error}`)
    const early = "Your last turn on this request ended early"
    if ((await pageText(run.page)).includes(early))
      failures.push("a usage limit resumed the turn")
    await expectShown(run.page, "Usage limit reached", failures)
    await expectShown(run.page, "Reset time unknown", failures)
    const box = run.page.getByRole("checkbox", {
      name: "Auto-continue this project's threads when limits reset",
    })
    await box.check()
    if (
      !(await waitFor(async () => {
        const view = await run.client.conversation.read(run.main.id)
        return view.usageLimitAutoContinue === true
      }, 15_000))
    )
      failures.push("the gateway did not turn usage-limit continuation on")
    await box.uncheck()
    if (
      !(await waitFor(async () => {
        const view = await run.client.conversation.read(run.main.id)
        return view.usageLimitAutoContinue === false
      }, 15_000))
    )
      failures.push("the gateway did not turn usage-limit continuation off")
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

  async drift(run, step, failures) {
    await run.exclusive(async () => {
      await run.restartConsole.during(run.opened, async () => {
        const before = failures.length
        // The composer is already on screen. The window has come back only
        // once its new session socket has received a frame.
        let stop = () => {}
        const recovered = new Promise((resolve) => {
          const onSocket = (socket) => {
            if (!socket.url().endsWith("/session")) return
            const received = () => resolve(true)
            stop = () => {
              socket.off("framereceived", received)
              run.page.off("websocket", onSocket)
            }
            socket.once("framereceived", received)
          }
          run.page.on("websocket", onSocket)
          stop = () => run.page.off("websocket", onSocket)
        })
        const old = run.client
        run.restarts += 1
        const drifted = mkdtempSync(join(tmpdir(), "nessa-stress-drift-"))
        const path = `/opt/homebrew/bin:${process.env.PATH ?? ""}`
        try {
          await run.stack.gateway.restart("SIGTERM", {
            overlay: {
              NESSA_AGENT_PATH: path,
              PATH: path,
              TMPDIR: drifted,
            },
            whileStopped: async () => moveStagedRuntime(run.stack.gateway.directory),
          })
          old.close()
          run.client = await connectClient(run.stack.gateway, INSTANCE, "panel")
          const back = await Promise.race([recovered, sleep(60_000).then(() => false)])
          if (!back)
            failures.push("the window did not come back after the agent path changed")
        } finally {
          stop()
        }
        return failures.length === before
      })
    })
    // The existing conversation still takes a message, in the window.
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async linking(run, step, failures) {
    await run.exclusive(async () => {
      await openLinkedDevices(run.page)
      if (!(await switchLinking(run, true, failures))) return
      if (!(await announced(run, failures))) return
      if (!(await pairFromWindow(run, failures))) return
      // Away and back: the tab reads linking again, and it is still on.
      await closeSettings(run.page)
      await openLinkedDevices(run.page)
      if (!(await switchLinking(run, false, failures))) return
      await closeSettings(run.page)
    })
    // The restarted gateway still answers the conversation, in the window.
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async github(run, step, failures) {
    const github = run.stack.github
    if (!github) {
      failures.push("the stack was started without the fake GitHub")
      return
    }
    const namespace = join(run.stack.gateway.directory, "ci", "instances", INSTANCE)
    const signIn = join(namespace, "github", "sign-in.json")
    const print = createHash("sha256").update(github.token).digest("hex").slice(0, 16)
    const row = run.page.locator(css.github)
    const state = () => row.getAttribute("data-github-state").catch(() => null)
    // Each state the person sees, kept with the evidence.
    const shot = (name) =>
      run.evidence
        ? run.page
            .screenshot({ path: join(run.evidence, `github-${name}.png`) })
            .catch(() => {})
        : undefined
    // GitHub's avatar host, answered here: which pictures were asked for.
    const avatars = []
    const pictures = `${GITHUB_AVATARS}/**`
    const answer = (route) => {
      avatars.push(route.request().url())
      return route.fulfill({ contentType: "image/png", body: FAKE_AVATAR })
    }
    await run.page.route(pictures, answer)
    // Answered until the step ends: its Disconnect reopens the signed-in row.
    try {
      await run.exclusive(async () => {
        await openAccounts(run.page)
        if (!(await waitFor(async () => (await state()) === "signed-out", 30_000))) {
          failures.push(`GitHub was ${await state()} before Connect`)
          return
        }
        await shot("signed-out")
        await row.getByRole("button", { name: names.githubConnect }).click()
        const code = row.locator("[data-github-code]")
        const shown = await waitFor(
          async () =>
            github.userCode() !== null &&
            (await code.getAttribute("data-github-code").catch(() => null)) ===
              github.userCode(),
          30_000,
        )
        if (!shown) {
          failures.push(
            `the row did not show the code GitHub handed out (${github.userCode()})`,
          )
          return
        }
        await shot("connecting")
        const open = await row.locator("[data-github-open]").getAttribute("href")
        if (open !== github.verificationUri)
          failures.push(`Open GitHub goes to ${open}, not ${github.verificationUri}`)
        // The person enters the code on GitHub: the gateway's next poll takes it.
        github.approve()
        const connected = await waitFor(
          async () =>
            (await state()) === "connected" &&
            (await row.textContent()).includes(`@${FAKE_ACCOUNT.login}`) &&
            (await row.locator(css.githubStatus).textContent()) === "Connected",
          30_000,
        )
        if (!connected) {
          failures.push(`the row is ${await state()} after GitHub approved the code`)
          return
        }
        // The picture is GitHub's, asked for by the account's id: the page was
        // answered above, so it loads here with nothing leaving the machine.
        const avatar = run.page.locator(`${css.githubAvatar} img`)
        const pictured = await waitFor(
          () =>
            avatar
              .evaluate((image) => image.complete && image.naturalWidth > 0)
              .catch(() => false),
          10_000,
        )
        if (!pictured) failures.push("the row did not draw the account's picture")
        if (!avatars.some((url) => url.includes(`/u/${FAKE_ACCOUNT.id}?`)))
          failures.push(
            `the picture was not asked for by id: ${avatars.join(", ") || "none"}`,
          )
        // At rest the actions are out of sight; the pointer or focus brings them.
        const opacity = (selector) =>
          row.locator(selector).evaluate((node) => getComputedStyle(node).opacity)
        const shows = (actions) =>
          waitFor(
            async () =>
              (await opacity(css.githubActions)) === (actions ? "1" : "0") &&
              (await opacity(css.githubStatus)) === (actions ? "0" : "1"),
            5_000,
          )
        await run.page.mouse.move(1, 1)
        if (!(await shows(false))) failures.push("the actions show with the row at rest")
        // Hidden is out of reach too: what is under Disconnect's spot at rest is not it.
        const hit = await row
          .getByRole("button", { name: names.githubDisconnect })
          .evaluate((node) => {
            const box = node.getBoundingClientRect()
            const under = document.elementFromPoint(
              box.x + box.width / 2,
              box.y + box.height / 2,
            )
            return Boolean(under?.closest(".settings-github-actions"))
          })
        if (hit) failures.push("the hidden actions take the pointer at rest")
        await shot("connected")
        await row.hover()
        if (!(await shows(true)))
          failures.push("the actions did not show under the pointer")
        await shot("connected-hover")
        await run.page.mouse.move(1, 1)
        if (!(await shows(false)))
          failures.push("the actions still show once the pointer leaves")
        // Both actions are in the tab order: Shift+Tab from Disconnect reaches
        // Choose repositories, and Tab goes back.
        const disconnect = row.getByRole("button", { name: names.githubDisconnect })
        const focused = (locator) =>
          locator.evaluate((node) => node === document.activeElement)
        await disconnect.focus()
        await run.page.keyboard.press("Shift+Tab")
        if (!(await focused(row.locator("[data-github-repositories]"))))
          failures.push("Shift+Tab from Disconnect did not reach Choose repositories")
        await run.page.keyboard.press("Tab")
        if (!(await focused(disconnect)))
          failures.push("Tab from Choose repositories did not reach Disconnect")
        if (!(await shows(true)))
          failures.push("the actions did not show with focus in the row")
        await shot("connected-focus")
        const repositories = await row
          .locator("[data-github-repositories]")
          .getAttribute("href")
        if (repositories !== github.installUrl)
          failures.push(
            `Choose repositories goes to ${repositories}, not ${github.installUrl}`,
          )
        await closeSettings(run.page)
      })
      if (failures.length) return
      if (!existsSync(signIn)) failures.push("the sign-in was not saved")
      else if ((lstatSync(signIn).mode & 0o077) !== 0)
        failures.push(
          `the sign-in file is mode ${(lstatSync(signIn).mode & 0o777).toString(8)}`,
        )
      if (run.stack.gateway.log().includes(github.token))
        failures.push("the gateway logged the GitHub token")
      // An agent opened now is started with the sign-in, for gh and for git.
      await githubAgent(run, step, " connected", `token=${print} git=${print}`, failures)
      await githubAgent(
        run,
        step,
        " connected",
        `token=${print} git=${print}`,
        failures,
        true,
      )
      if (failures.length) return
      await run.exclusive(async () => {
        await openAccounts(run.page)
        // The actions are out of reach until the pointer is over the row, as a person's is.
        await row.hover()
        await row.getByRole("button", { name: names.githubDisconnect }).click()
        if (!(await waitFor(async () => (await state()) === "signed-out", 30_000)))
          failures.push(`the row is ${await state()} after Disconnect`)
        await closeSettings(run.page)
      })
      if (failures.length) return
      if (existsSync(signIn)) failures.push("Disconnect left the sign-in file")
      await githubAgent(run, step, " disconnected", "token=none git=none", failures)
      await githubAgent(run, step, " disconnected", "token=none git=none", failures, true)
      // Each step of both is on record, and no record holds the token.
      const audit = join(namespace, "conversations", "audit", "github")
      const records = existsSync(audit) ? readdirSync(audit) : []
      for (const event of [
        "connect-requested",
        "connected",
        "disconnect-requested",
        "disconnected",
      ])
        if (!records.some((name) => name.endsWith(`-${event}.json`)))
          failures.push(`the audit has no ${event} record`)
      if (
        records.some((name) =>
          readFileSync(join(audit, name), "utf8").includes(github.token),
        )
      )
        failures.push("an audit record holds the GitHub token")
    } finally {
      await run.page.unroute(pictures, answer)
    }
  },

  async upgrade(run, step, failures) {
    const listed = async () =>
      (await run.client.conversation.list({})).conversations
        .map((each) => each.conversationId)
        .sort()
    await run.exclusive(async () => {
      const before = await listed()
      const path = metadataFile(run.stack.gateway.directory)
      let older
      await run.restartConsole.during(run.opened, async () => {
        const count = failures.length
        const old = run.client
        run.restarts += 1
        await run.stack.gateway.restart("SIGTERM", async () => {
          older = asOlderBuild(path)
        })
        old.close()
        run.client = await connectClient(run.stack.gateway, INSTANCE, "panel")
        log(`upgrade: restarted on a version ${older} file`)
        const version = schemaVersion(path)
        if (version !== CURRENT_METADATA_VERSION)
          failures.push(
            `the gateway left the version ${older} file at ${version}, not ${CURRENT_METADATA_VERSION}`,
          )
        const after = await listed()
        if (JSON.stringify(after) !== JSON.stringify(before))
          failures.push(
            `the upgraded gateway lists ${after.length} conversations, not the ${before.length} it did`,
          )
        // The upgraded gateway still answers the conversation, in the
        // window: that answer comes over the window's new connection, so
        // the refused connections while it was down are the restart's.
        if (failures.length === count) await steps.reply(run, step, failures)
        return failures.length === count
      })
    })
  },

  async launch(run, step, failures) {
    await run.exclusive(async () => {
      const page = run.page
      const removeKey = () =>
        page.evaluate((key) => sessionStorage.removeItem(key), STARTING.key)
      // The window opens while the host is still starting the gateway, as at
      // every launch: the host holds its read until the gateway is up.
      await page.evaluate((key) => sessionStorage.setItem(key, "1"), STARTING.key)
      let covered
      let askedWhileStarting
      try {
        await page.reload()
        // Whatever the window shows meanwhile, it is never the startup
        // screen: the gateway did not fail, it is starting.
        covered = await page
          .locator(css.startupScreen)
          .waitFor({ timeout: LAUNCH_STARTING_MS })
          .then(() => true)
          .catch(() => false)
        // And the window did ask while it was starting: the hold was met.
        askedWhileStarting = await page.evaluate(
          () => window.__fakeHostAsked.load_gateway_endpoint,
        )
      } finally {
        // Up, whatever happened: a later reload must not find it starting.
        // A page mid-navigation refuses the evaluate; the next one may not.
        await removeKey().catch(() => removeKey().catch(() => {}))
      }
      if (covered) {
        failures.push(
          "the window showed the startup screen while the gateway was starting",
        )
        return
      }
      if (!(askedWhileStarting > 0)) {
        failures.push("the window did not ask for the gateway while it was starting")
        return
      }
      const started = Date.now()
      const listed = await page
        .locator(selectorFor.sessionListRowFor(run.main.id))
        .waitFor({ timeout: LAUNCH_LISTED_MS })
        .then(() => true)
        .catch(() => false)
      if (!listed) {
        failures.push(
          `the window did not list the main conversation within ${LAUNCH_LISTED_MS}ms of the gateway being up`,
        )
        return
      }
      log(`launch: listed ${Date.now() - started}ms after the gateway was up`)
      if ((await page.locator(css.startupScreen).count()) > 0) {
        failures.push("the startup screen covers the window after the gateway was up")
        return
      }
      await openStressConversation(page, run.main.id, run.main.title)
    })
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async menuPanel(run, step, failures) {
    const page = await panelWindow(run)
    try {
      const strip = page.locator(css.panelTabs)
      if (
        !(await strip.waitFor({ timeout: 30_000 }).then(
          () => true,
          () => false,
        ))
      ) {
        failures.push("the panel drew no tab strip")
        return
      }
      if ((await strip.getAttribute("data-variant")) !== "dots")
        failures.push("the panel's tabs are not page dots")
      // Messages, then the main conversation from its row: a chat opened
      // from the list takes a dot, and its name leads the strip.
      await page.locator(css.panelMessages).click()
      const rowFor = (title) =>
        page.locator(css.panelListRow).filter({ hasText: title }).first()
      if (
        !(await rowFor(run.main.title)
          .waitFor({ timeout: 15_000 })
          .then(
            () => true,
            () => false,
          ))
      ) {
        failures.push(
          `the panel's Messages does not list ${JSON.stringify(run.main.title)}`,
        )
        return
      }
      const dotsBefore = await page.locator(css.panelDot).count()
      await rowFor(run.main.title).click()
      await panelNamed(page, run.main.title, failures)
      const dots = await page.locator(css.panelDot).count()
      if (dots !== dotsBefore + 1)
        failures.push(`opening a chat left ${dots} dots, not ${dotsBefore + 1}`)
      // The selected dot is the bar: wider than every other dot, once its
      // width has settled.
      const widths = () =>
        page.locator(css.panelDot).evaluateAll((all) =>
          all.map((dot) => ({
            width: dot.getBoundingClientRect().width,
            selected: dot.closest("[data-active]") !== null,
          })),
        )
      const oneBar = (all) => {
        const bar = all.filter((each) => each.selected)
        return (
          bar.length === 1 &&
          all.every((each) => each.selected || each.width + 6 <= bar[0].width)
        )
      }
      if (!(await waitFor(async () => oneBar(await widths()), 5_000)))
        failures.push(
          `the selected dot is not the one bar: ${JSON.stringify(await widths())}`,
        )
      // A message sent from the panel wears the chat accent and its rim.
      const text = prompt(run, STRESS.reply, step, " from the panel")
      const seen = await seenTurns(run, run.main.id)
      const box = page.locator(css.panelComposer)
      await box.click()
      await page.keyboard.type(text)
      await page.keyboard.press("Enter")
      run.sent.set(run.main.id, (run.sent.get(run.main.id) ?? 0) + 1)
      await settled(run, run.main.id, seen, text, "completed", failures)
      const bubble = page.locator(css.sentBubble).filter({ hasText: text }).first()
      if (
        !(await bubble.waitFor({ timeout: 15_000 }).then(
          () => true,
          () => false,
        ))
      ) {
        failures.push("the panel does not show the message it sent")
      } else {
        // Its colours are nessa_ui's to choose: the bubble must wear the chat
        // accent and its rim, whatever values the theme gives them.
        const paint = await bubble.evaluate((node) => {
          const probe = document.createElement("span")
          probe.style.background = "var(--nessa-chat-accent)"
          probe.style.boxShadow = "var(--nessa-chat-accent-rim)"
          node.parentElement.append(probe)
          const want = getComputedStyle(probe)
          const style = getComputedStyle(node)
          const paint = {
            background: style.backgroundColor,
            accent: want.backgroundColor,
            shadow: style.boxShadow,
            rim: want.boxShadow,
          }
          probe.remove()
          return paint
        })
        if (
          paint.accent === "rgba(0, 0, 0, 0)" ||
          paint.rim === "none" ||
          paint.background !== paint.accent ||
          !paint.shadow.includes(paint.rim)
        )
          failures.push(
            `a sent bubble does not wear the chat accent and its rim: ${JSON.stringify(paint)}`,
          )
      }
      // The panel has no workspace, so its reply is looked for on the page.
      const echo = `${SAYS.echo}${text}`
      if (
        !(await page
          .getByText(echo)
          .first()
          .waitFor({ timeout: 30_000 })
          .then(
            () => true,
            () => false,
          ))
      )
        failures.push(`the panel does not show ${JSON.stringify(echo)}`)
      // A chat archived from Messages leaves a row card with its avatar,
      // and the card's Undo brings it back, in the list and the gateway.
      const conversationId = randomUUID()
      const first = prompt(run, STRESS.reply, step, " to archive")
      await run.client.conversation.create({ conversationId, agent: run.stack.agent })
      await run.client.conversation.submit(
        run.client.conversation.prepare(conversationId, first),
      )
      run.checked.push(conversationId)
      await settled(run, conversationId, new Set(), first, "completed", failures)
      const title = await titleOf(run, conversationId)
      await page.locator(css.panelMessages).click()
      const row = rowFor(title)
      if (
        !(await row.waitFor({ timeout: 15_000 }).then(
          () => true,
          () => false,
        ))
      ) {
        failures.push(`the panel's Messages does not list ${JSON.stringify(title)}`)
        return
      }
      await row.focus()
      // From the keyboard: a pointer click races the row sliding open, and a
      // list that redraws under it. Shift+F10 opens the actions and focuses
      // the first; Tab walks them.
      await page.keyboard.press("Shift+F10")
      await waitFor(
        () =>
          page.evaluate(
            () =>
              document.activeElement?.getAttribute("data-slot") ===
              "conversation-history-row-action",
          ),
        5_000,
      )
      const onArchive = () =>
        page.evaluate(
          (name) => document.activeElement?.getAttribute("aria-label") === name,
          `Archive ${title}`,
        )
      for (let tabs = 0; tabs < 4 && !(await onArchive()); tabs++)
        await page.keyboard.press("Tab")
      if (!(await onArchive())) {
        failures.push(
          `Shift+F10 on ${JSON.stringify(title)} does not reach its Archive action`,
        )
        return
      }
      await page.keyboard.press("Enter")
      const undoCard = page.locator(css.undoCard)
      if (
        !(await undoCard.waitFor({ timeout: 15_000 }).then(
          () => true,
          () => false,
        ))
      ) {
        failures.push("archiving from the panel left no undo card")
        return
      }
      if (!(await undoCard.innerText()).includes(title))
        failures.push(`the undo card does not name ${JSON.stringify(title)}`)
      if ((await undoCard.locator(css.randomAvatar).count()) !== 1)
        failures.push("the undo card does not lead with the conversation's avatar")
      const archived = async (want) =>
        waitFor(async () => {
          const { conversations } = await run.client.conversation.list({ archived: true })
          return (
            conversations.some((each) => each.conversationId === conversationId) === want
          )
        }, 15_000)
      if (!(await archived(true)))
        failures.push("the gateway did not archive the conversation")
      await undoCard.getByRole("button", { name: /^Undo/ }).click()
      if (!(await archived(false)))
        failures.push("Undo did not unarchive the conversation")
      if (
        !(await row.waitFor({ timeout: 15_000 }).then(
          () => true,
          () => false,
        ))
      )
        failures.push("Undo did not put the row back in the panel's Messages")
      if (!(await waitFor(async () => (await undoCard.count()) === 0, 5_000)))
        failures.push("the undo card stayed after Undo")
    } finally {
      await page.close()
    }
  },

  async rail(run, step, failures) {
    await run.exclusive(async () => {
      await setSideRail(run.page, true, failures)
      if (failures.length) return
      const toggle = run.page.locator(css.sideRailToggle)
      if ((await toggle.getAttribute("aria-disabled")) === "true") {
        failures.push("the window had no room for the side rail")
        return
      }
      // Open, closed, open again. Each open sends its own phrase: settled
      // counts copies of one phrase, and the first send is already there.
      let opened = 0
      let place
      for (const open of [true, false, true]) {
        if ((await toggle.getAttribute("aria-expanded")) !== String(open))
          failures.push(...(await footerThroughToggle(run.page)))
        if (!(await aroundTheLights(run.page, open ? "open" : "closed", failures))) return
        // The toggle stays under the pointer: opening or closing the rail
        // moves what is beside it, never the toggle itself.
        const box = await toggle.boundingBox()
        const at = box && [box.x, box.y, box.width, box.height].map(Math.round).join()
        place ??= at
        if (at !== place)
          failures.push(
            `the rail's toggle moved to [${at}] with the rail ${open ? "open" : "closed"}, from [${place}]`,
          )
        // The workspace still takes a message with the rail beside it.
        if (open && failures.length === 0) {
          opened += 1
          await steps.reply(
            run,
            opened === 1 ? step : { ...step, extra: " again" },
            failures,
          )
        }
      }
      await studioWithTheCorner(run.page, failures)
      // Turned off with the rail open: the rail goes, and back from Settings
      // the titlebar's controls start where the lights end again.
      await setSideRail(run.page, false, failures)
      await aroundTheLights(run.page, "off", failures)
    })
  },

  async panel(run, step, failures) {
    await run.exclusive(async () => {
      const page = run.page
      const panel = page.locator("#workspace-panel")
      const grid = () =>
        page.evaluate(
          () =>
            document.querySelector(".split-panes-grid")?.getBoundingClientRect().right ??
            0,
        )
      const before = await grid()
      await page.keyboard.press("Meta+Alt+KeyB")
      const opened = await waitFor(
        async () => (await panel.getAttribute("aria-hidden")) === null,
        3000,
      )
      if (!opened) {
        failures.push("⌥⌘B did not open the right panel")
        return
      }
      await page.waitForTimeout(500)
      // The panes give the panel its room; the conversation still takes a message.
      const beside = await grid()
      if (!(beside < before - 100))
        failures.push(`the panes did not make room for the panel: ${before} → ${beside}`)
      await steps.reply(run, step, failures)
      await page.getByRole("button", { name: "Expand Panel" }).click()
      if (
        !(await waitFor(
          async () => (await panel.getAttribute("data-maximized")) !== null,
          2000,
        ))
      )
        failures.push("Expand Panel did not maximize it")
      await expandedForItsChat(run, failures)
      await page.getByRole("button", { name: "Expand Panel" }).click()
      if (
        !(await waitFor(
          async () => (await panel.getAttribute("data-maximized")) !== null,
          2000,
        ))
      )
        failures.push("Expand Panel did not maximize it again")
      await page.keyboard.press("Escape")
      if (
        !(await waitFor(
          async () => (await panel.getAttribute("data-maximized")) === null,
          2000,
        ))
      )
        failures.push("Escape did not restore the maximized panel")
      await page.keyboard.press("Meta+Alt+KeyB")
      if (
        !(await waitFor(
          async () => (await panel.getAttribute("aria-hidden")) === "true",
          3000,
        ))
      )
        failures.push("⌥⌘B did not close the right panel")
      await page.waitForTimeout(500)
      const after = await grid()
      if (Math.abs(after - before) > 1)
        failures.push(
          `the panes did not take the panel's room back: ${before} → ${after}`,
        )
    })
  },

  async onboarding(run, step, failures) {
    await run.exclusive(async () => {
      const asked = await replayOnboarding(run.page, run.evidence)
      if (asked !== 1) {
        failures.push(`Replay asked the host ${asked} times, not once`)
        return
      }
      // The host opens the setup window for that ask; this page is that
      // window, over the same gateway, as the real host builds it. Both
      // agents are ready, so a click on either is a choice.
      const page = await setupWindow(run, (page) =>
        agentsProbe(page, () => ({ claude: "ready", codex: "ready" })),
      )
      try {
        const heading = (name) => page.getByRole("heading", { name, exact: true })
        const button = (name) => page.getByRole("button", { name, exact: true })
        await heading(names.setup.welcome).waitFor({ timeout: 15_000 })
        await openedAsAGlow(page, failures)
        // Welcome has nothing before it: no Back, and Close in its corner.
        await button(names.setup.close).waitFor()
        let corners = await setupCorners(page)
        if (corners.back) failures.push("welcome shows a Back button")
        if (!inCorner(corners.close, "right"))
          failures.push(
            `Close is not round in the top-right corner: ${JSON.stringify(corners.close)}`,
          )
        // Forward and back twice: each time the picker has Back in the
        // top-left corner, and it returns to welcome.
        for (let round = 0; round < 2; round += 1) {
          await button(names.setup.getStarted).click()
          await heading(names.setup.chooseAgent).waitFor({ timeout: 10_000 })
          await button(names.setup.back).waitFor()
          corners = await setupCorners(page)
          if (!inCorner(corners.back, "left"))
            failures.push(
              `Back is not round in the top-left corner: ${JSON.stringify(corners.back)}`,
            )
          if (!inCorner(corners.close, "right"))
            failures.push(
              `on the picker Close left its corner: ${JSON.stringify(corners.close)}`,
            )
          await button(names.setup.back).click()
          await heading(names.setup.welcome).waitFor({ timeout: 10_000 })
          if (await button(names.setup.back).count())
            failures.push("back on welcome, the Back button is still shown")
        }
        // The picker lists the agents setup offers, each with its figure.
        await button(names.setup.getStarted).click()
        await heading(names.setup.chooseAgent).waitFor({ timeout: 10_000 })
        await pickerOffers(page, failures)
        await page.screenshot({ path: join(run.evidence, "onboarding-picker.png") })
        // Close leaves setup from the picker: one handoff, as left.
        await button(names.setup.close).click()
        const handed = await waitFor(
          async () =>
            (await page.evaluate(() => window.__fakeHostSetupFinishes.length)) > 0,
          10_000,
        )
        const finishes = await page.evaluate(() => window.__fakeHostSetupFinishes)
        if (!handed) failures.push("Close did not hand setup back to the host")
        else if (finishes.length !== 1 || finishes[0].completed !== false)
          failures.push(
            `Close handed over ${JSON.stringify(finishes)}, not one left setup`,
          )
        if (await page.locator(".nessa-setup-window").count())
          failures.push("setup is still on screen after Close")
      } finally {
        await page.close()
      }
      await finishedIntoTheMenuBar(run, failures)
    })
    // The window is still the window it was: a message sent there is answered.
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async pool(run, step, failures) {
    // Workers dial the gateway's native listener, which is off in this run
    // until a step turns it on: a person does it in Settings, here as in
    // the `linking` step, and turns it off again once the workers are gone.
    const linked = await run.exclusive(async () => {
      await openLinkedDevices(run.page)
      const on = await switchLinking(run, true, failures)
      if (on) await closeSettings(run.page)
      return on
    })
    if (!linked) return
    const workers = []
    let owner
    try {
      // Pairing is the owner's: the panel's credential cannot approve.
      owner = await connectClient(run.stack.gateway, INSTANCE, "owner")
      const launch = scriptedLaunch(run.stack.agent, { scenario: run.stack.scenario })
      for (const label of ["a", "b"])
        workers.push(
          await startWorker({
            owner,
            address: run.nativeAddress,
            agent: run.stack.agent,
            launch,
            label: `${label}${step.index}`,
            connected: async (name) =>
              (await offers(run, (environments) => environments.includes(name))).includes(
                name,
              ),
          }),
        )
      const offered = await offers(run, (environments) =>
        workers.every((each) => environments.includes(each.name)),
      )
      if (offered?.[0] !== POOL) {
        failures.push(
          `the gateway offers ${JSON.stringify(offered)}, not ${POOL} first and both workers`,
        )
        return
      }
      const conversationId = randomUUID()
      await run.client.conversation.create({
        conversationId,
        agent: run.stack.agent,
        environment: POOL,
      })
      const first = prompt(run, STRESS.reply, step, " pool")
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
      const bound = bindingOf(run, conversationId)
      const from = workers.find((each) => each.name === bound?.worker)
      const to = workers.find((each) => each !== from)
      if (!from || !bound.context) {
        failures.push(
          `the pool conversation is bound as ${JSON.stringify(bound)}, not to a worker with its context`,
        )
        return
      }
      if (before.lease?.environment !== "peer" || before.lease?.host !== from.name)
        failures.push(`its lease runs on ${before.lease?.host}, not ${from.name}`)
      // A person opens it and writes in it: it stays on the worker it is bound to.
      await openStressConversation(
        run.page,
        conversationId,
        await titleOf(run, conversationId),
      )
      await expectShown(run.page, await placeOf(from), failures)
      const stays = prompt(run, STRESS.reply, step, " pool stays")
      const kept = await send(
        run,
        conversationId,
        stays,
        failures,
        new Set(before.messages.map((each) => each.executionId)),
      )
      const { view: still } = await settled(
        run,
        conversationId,
        kept,
        stays,
        "completed",
        failures,
      )
      await expectShown(run.page, `${SAYS.echo}${stays}`, failures)
      if (JSON.stringify(bindingOf(run, conversationId)) !== JSON.stringify(bound))
        failures.push(
          `a message moved the conversation off its serving worker: ${JSON.stringify(bindingOf(run, conversationId))}`,
        )
      if (failures.length) return
      // Its worker is stopped the way a container runtime stops it: it
      // drains, ends its lease, and exits on its own.
      const drainedAt = Date.now()
      if (!(await from.stop("SIGTERM", 30_000))) {
        failures.push(`worker ${from.label} did not drain and exit within 30 s`)
        return
      }
      if (from.process.exitCode !== 0)
        failures.push(
          `worker ${from.label} exited ${from.process.exitCode ?? from.process.signalCode} after draining`,
        )
      log(`pool: worker ${from.label} drained in ${Date.now() - drainedAt} ms`)
      const left = await offers(
        run,
        (environments) =>
          !environments.includes(from.name) && environments.includes(to.name),
      )
      if (!left || left[0] !== POOL)
        failures.push(`after the drain the gateway offers ${JSON.stringify(left)}`)
      // The gateway's view of it, once the drain's end of its lease is
      // there. A view asked while the gateway closes the agent that lease
      // ran is refused, as a read across a restart is; the next one is not.
      const lease = await waitFor(async () => {
        try {
          const { lease } = await followedOnce(run, conversationId)
          return lease?.state === "ended" ? lease : null
        } catch (error) {
          log(`view ${conversationId}: ${explain(error)}`)
          return null
        }
      }, 30_000)
      if (
        lease?.state !== "ended" ||
        lease.cause !== "stopped" ||
        lease.host !== from.name
      )
        failures.push(`the drained lease is ${JSON.stringify(lease)}, not ended stopped`)
      // The next message moves it: another worker, a new provider context.
      const moves = prompt(run, STRESS.reply, step, " pool moves")
      const moved = await send(
        run,
        conversationId,
        moves,
        failures,
        new Set(still?.messages.map((each) => each.executionId)),
      )
      const { view: after } = await settled(
        run,
        conversationId,
        moved,
        moves,
        "completed",
        failures,
      )
      const rebound = bindingOf(run, conversationId)
      if (rebound?.worker !== to.name)
        failures.push(`after the drain it is bound to ${rebound?.worker}, not ${to.name}`)
      if (!rebound?.context || rebound.context === bound.context)
        failures.push(
          `after the move its provider context is ${rebound?.context}, not a new one`,
        )
      if (failures.length) return
      await expectShown(run.page, `${SAYS.echo}${moves}`, failures)
      await expectShown(run.page, await placeOf(to), failures)
      if (after?.lease?.state !== "live" || after.lease.host !== to.name)
        failures.push(
          `after the move its lease is ${after?.lease?.state} on ${after?.lease?.host}`,
        )
      // With no worker serving, the gateway refuses to read it
      // (`environment_not_configured`), so it is checked at rest here, while
      // one serves, and not in `run.checked` for the run's own rest check.
      failures.push(...(await atRest(run, [conversationId])))
      const stopped = Date.now()
      if (!(await to.stop("SIGTERM", 30_000)))
        failures.push(`worker ${to.label} did not drain and exit within 30 s`)
      else if (to.process.exitCode !== 0)
        failures.push(
          `worker ${to.label} exited ${to.process.exitCode ?? to.process.signalCode} after draining`,
        )
      log(`pool: worker ${to.label} drained in ${Date.now() - stopped} ms`)
      await openStressConversation(run.page, run.main.id, run.main.title)
    } finally {
      // Nothing the step started outlives it, whatever went wrong.
      for (const each of workers) {
        if (run.evidence)
          writeFileSync(join(run.evidence, `worker-${each.label}.log`), each.log())
        await each.close()
      }
      owner?.close()
      await run.exclusive(async () => {
        await openLinkedDevices(run.page)
        if (await switchLinking(run, false, failures)) await closeSettings(run.page)
      })
    }
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
    else {
      await expectShown(run.page, SAYS.report, failures)
      await reportLine(run, report, failures)
    }
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
    await flatPanes(run.page, failures)
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
    // One header per chat, in the titlebar's row — beside the session list,
    // and with the panes alone at the window's edge.
    await headersLand(run.page, "beside the session list", failures)
    await run.page.keyboard.press(keys.toggleSessionList)
    await waitFor(
      async () =>
        (await run.page.locator(`${css.workspace}[data-panes-alone]`).count()) === 1,
      5_000,
    )
    await headersLand(run.page, "with the panes alone", failures)
    await run.page.keyboard.press(keys.toggleSessionList)
    await waitFor(
      async () =>
        (await run.page.locator(`${css.workspace}[data-panes-alone]`).count()) === 0,
      5_000,
    )
    if (failures.length) return
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

  async webhook(run, step, failures) {
    // The panel creates this conversation and holds the default owner grants,
    // including credential.manage. Sharing must use that same conversation owner.
    const owner = await connectClient(run.stack.gateway, INSTANCE, "panel")
    const conversationId = randomUUID()
    let shared = false
    try {
      if (!run.stack.webhookCredential) {
        const { organizationId, gatewayId } = await owner.auth.session()
        const principalId = "stress-webhook"
        const issued = await owner.credentials.issue({
          principal: { id: principalId, kind: "integration" },
          membership: {
            id: "stress-webhook-membership",
            principalId,
            organizationId,
            role: "member",
            state: "active",
          },
          expiresAt: null,
          grants: [
            { action: "conversation.read", resource: { organizationId, id: gatewayId } },
          ],
        })
        if (!("secretUnavailable" in issued) || issued.secretUnavailable !== true)
          throw new Error("webhook issue must withhold its synthetic credential secret")
        run.stack.webhookCredential = issued.credential.id
      }
      const credential = run.stack.webhookCredential
      await run.client.conversation.create({ conversationId, agent: run.stack.agent })
      run.checked.push(conversationId)
      const setup = prompt(run, STRESS.reply, step, " webhook setup")
      await run.client.conversation.submit(
        run.client.conversation.prepare(conversationId, setup),
      )
      await settled(run, conversationId, new Set(), setup, "completed", failures)
      if (failures.length) return
      await owner.conversation.share(conversationId, credential, ["read"])
      shared = true
      const sameScope = (a, b) =>
        ["receiver", "origin", "stream", "incarnation", "schema", "accessEpoch"].every(
          (key) => a[key] === b[key],
        )
      const catchUp = async (fixed) => {
        if (
          !(await waitFor(async () => {
            const native = await owner.conversation.webhookCursor(
              conversationId,
              credential,
            )
            if (native.resetRequired || !sameScope(native.scope, fixed.scope))
              throw new Error(
                "webhook capture changed incarnation or requires explicit reset",
              )
            return BigInt(native.position) >= BigInt(fixed.head)
          }, 15_000))
        )
          throw new Error("webhook native cursor did not reach the fixed source Head")
      }
      // The first source cycle may not yet have installed a cursor. Poll the
      // native diagnostic only; a bounded refusal is not treated as caught up.
      let lastRefusal
      if (
        !(await waitFor(async () => {
          try {
            await owner.conversation.webhookCursor(conversationId, credential)
            return true
          } catch (error) {
            if (error.code !== "invalid_request") throw error
            lastRefusal = error
            return false
          }
        }, 15_000))
      )
        throw new Error("webhook cursor was never installed", { cause: lastRefusal })
      const baseline = (
        await owner.conversation.webhookCursor(conversationId, credential, {
          includeSourceHead: true,
        })
      ).observedSourceHead
      await catchUp(baseline)
      const title = await titleOf(run, conversationId)
      await openStressConversation(run.page, conversationId, title)
      const text = prompt(run, STRESS.webhook, step)
      const seen = await send(run, conversationId, text, failures)
      await settled(run, conversationId, seen, text, "completed", failures)
      await expectShown(run.page, `${SAYS.webhook}${text}`, failures)
      if (failures.length) return
      // Capture once after this turn commits. Poll only the native cursor;
      // observing newer Heads on each iteration would chase a moving target.
      const fixed = (
        await owner.conversation.webhookCursor(conversationId, credential, {
          includeSourceHead: true,
        })
      ).observedSourceHead
      if (
        !sameScope(baseline.scope, fixed.scope) ||
        BigInt(fixed.head) <= BigInt(baseline.head)
      )
        throw new Error(
          "browser webhook witness did not advance the same source incarnation",
        )
      await catchUp(fixed)
      const proof = webhookNoAttempt(run.stack.gateway, INSTANCE, credential, fixed)
      log(
        `webhook captured fixed Head ${fixed.head}; ${proof.queued} queued, ${proof.attempted} attempted; loopback policy unchanged`,
      )
    } finally {
      try {
        if (shared)
          await owner.conversation.unshare(conversationId, run.stack.webhookCredential)
      } catch (error) {
        failures.push(`webhook grant cleanup failed: ${error.message}`)
      } finally {
        owner.close()
        await openStressConversation(run.page, run.main.id, run.main.title)
      }
    }
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

  async channel(run, step, failures) {
    const notes = `Answer as the stress channel s${run.knobs.seed}.`
    // In the run's own folder, removed with it: the rest check reads these
    // channels' conversations again at the end, and an agent cannot be built
    // in a folder that is gone.
    const attached = mkdtempSync(join(run.stack.scratch, "channel-"))
    const resource = join(attached, "brief.md")
    writeFileSync(resource, "# Brief\n")
    const chosen = mkdtempSync(join(run.stack.scratch, "chosen-"))
    try {
      // Made in a folder of its own, after a folder agents may not use.
      const made = await makeChannel(run, failures, {
        name: `stress-made-r${step.index}-s${run.knobs.seed}`,
        refusedFolder: "/",
        notes,
        resource,
      })
      if (made) {
        if (!made.folderMade || !made.folder.includes(`/channels/${made.channelId}`))
          failures.push(
            `the channel made without a folder works in ${made.folder}, not one made for it`,
          )
        if (!existsSync(made.folder))
          failures.push(`the made folder ${made.folder} is not there`)
        await talkInChannel(run, step, made, failures, notes)
      }
      // Made in a folder chosen in the host's picker.
      const there = await makeChannel(run, failures, {
        name: `stress-chosen-r${step.index}-s${run.knobs.seed}`,
        folder: chosen,
      })
      if (there) {
        if (there.folderMade || there.folder !== realpathSync(chosen))
          failures.push(
            `the channel made in ${chosen} works in ${there.folder} (made: ${there.folderMade})`,
          )
        await talkInChannel(run, step, there, failures)
      }
    } finally {
      // Back to the main conversation, in the default channel.
      await run.page
        .locator(`${css.sidebar} nav`)
        .getByText("Conversations", { exact: true })
        .first()
        .click({ timeout: 10_000 })
        .catch(() => {})
      await openStressConversation(run.page, run.main.id, run.main.title)
    }
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

  async notice(run, step, failures) {
    const conversationId = randomUUID()
    run.checked.push(conversationId)
    await run.client.conversation.create({
      conversationId,
      agent: run.stack.agent,
      approvalMode: "ask",
    })
    // Named by a first turn: the gateway records a conversation's title just
    // after the agent takes its first message, and the scripted agent can
    // ask before that lands, which a model never does.
    const naming = prompt(run, STRESS.reply, step, " notice")
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, naming),
    )
    const { view: named } = await settled(
      run,
      conversationId,
      new Set(),
      naming,
      "completed",
      failures,
    )
    if (failures.length) return
    const title = await titleOf(run, conversationId)
    // The person is on the main conversation: the review waits where the
    // window does not show it, and only the notice says so.
    const first = prompt(run, STRESS.permission, step, " notice")
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    const posted = await noticePosted(run, conversationId, failures)
    if (!posted) return
    if (posted.notice.body !== title)
      failures.push(
        `the notice names ${JSON.stringify(posted.notice.body)}, not the conversation ${JSON.stringify(title)}`,
      )
    if (posted.notice.title !== `${posted.review.toolName} needs a decision`)
      failures.push(
        `the notice says ${JSON.stringify(posted.notice.title)}, not that ${posted.review.toolName} needs a decision`,
      )
    // The person clicks the notice: the host sends its event and the window
    // opens that conversation on its card.
    const listening = await run.page.evaluate(
      (id) => window.__fakeHostOpenNotice?.(id) ?? 0,
      conversationId,
    )
    if (listening !== 1)
      failures.push(`${listening} windows listened for an opened notice, not 1`)
    const opened = await run.page
      .locator(`${selectorFor.sessionListRowFor(conversationId)}[aria-selected=true]`)
      .waitFor({ timeout: 15_000 })
      .then(() => true)
      .catch(() => false)
    if (!opened) {
      failures.push("opening the notice did not bring its conversation up")
      return
    }
    await answerCard(run, conversationId, "allow", failures, "Report the rows")
    await noticeTakenBack(run, posted.notice, "answered in the window", failures)
    const { view: before } = await settled(
      run,
      conversationId,
      new Set(named?.messages.map((each) => each.executionId) ?? []),
      first,
      "completed",
      failures,
    )
    await expectShown(run.page, SAYS.allowed, failures)
    if (failures.length) return
    // Back on the main conversation, a second review there is answered by
    // another of the person's clients: the window was never shown it, and
    // its notice still goes.
    await openStressConversation(run.page, run.main.id, run.main.title)
    const second = prompt(run, STRESS.permission, step, " notice elsewhere")
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, second),
    )
    const again = await noticePosted(run, conversationId, failures, posted.notice)
    if (!again) return
    // The window is reloaded while that review waits. The gateway sends the
    // waiting set as soon as the new socket is up, before the window listens;
    // the window must still hand it to the host, which keeps the notice up.
    await run.page.reload()
    await run.page
      .locator(selectorFor.sessionListRowFor(run.main.id))
      .waitFor({ timeout: 30_000 })
    const kept = await waitFor(async () => {
      const notices = await hostNotices(run.page)
      return (
        notices !== null &&
        notices.syncs > 0 &&
        notices.up.some((each) => sameNotice(each, again.notice))
      )
    }, 15_000)
    if (!kept) {
      failures.push(
        "after a reload the window did not hand the waiting review to the host",
      )
      return
    }
    await openStressConversation(run.page, run.main.id, run.main.title)
    const { permissionTargets } = await import("@nessa/client")
    const view = await readWithReviews(run.client, conversationId)
    const index = view.permissions.findIndex(
      (each) => each.permissionId === again.review.permissionId,
    )
    const deny = view.permissions[index]?.options.find((each) => each.effect === "deny")
    if (!deny) {
      failures.push("the second review offers no deny")
      return
    }
    await run.client.conversation.answer(permissionTargets(view)[index], deny.id)
    await noticeTakenBack(run, again.notice, "answered elsewhere", failures)
    await settled(
      run,
      conversationId,
      new Set(before?.messages.map((each) => each.executionId) ?? []),
      second,
      "completed",
      failures,
    )
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

  /**
   * A conversation switched to the other agent from the composer's picker,
   * as a person does: the pick is only staged (taken back once, then picked
   * again), and the next message sent makes the switch. Then forked from
   * its row's More menu. Each is talked
   * to after: the first turn of each new session is the one given the
   * earlier turns (`SAYS.carried`), and it runs on the agent and model
   * asked for. The window draws a line at the switch, and the fork opens
   * on the copied turns and a line after them.
   */
  async handoff(run, step, failures) {
    const other = otherAgent(run.stack.agent)
    const target = catalog.models.find((model) => model.modelId === MODELS[other])
    if (!target) throw new CannotRun(`the catalog has no ${MODELS[other]}`)
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " handoff")
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
    if (failures.length || !before) return
    await openStressConversation(
      run.page,
      conversationId,
      await titleOf(run, conversationId),
    )
    // Picking another provider's model only stages it: nothing is switched,
    // and no line is drawn, until a message is sent. Picking the
    // conversation's own model again takes the pick back.
    const picker = dock(run.page).locator(css.modelChip)
    const own = catalog.models.find((model) => model.modelId === before.model)
    if (!own) throw new CannotRun(`the catalog has no ${before.model}`)
    const line = run.page.locator(`${css.handoffLine}[data-handoff="switch"]`)
    const unswitched = async (what) => {
      if (
        !(await waitFor(
          async () =>
            (await picker.getAttribute("aria-label"))?.includes(what.displayName),
          5_000,
        ))
      )
        failures.push(`the picker did not show ${what.displayName} once picked`)
      const moved = await waitFor(async () => {
        const view = await run.client.conversation.read(conversationId)
        return view.model !== before.model || view.handoffs.length > 0
      }, 2_000)
      if (moved)
        failures.push(
          `picking ${what.displayName} switched the conversation before a send`,
        )
      if (await line.count())
        failures.push(`picking ${what.displayName} drew a switch line before a send`)
    }
    await pickModel(run.page, picker, target)
    await unswitched(target)
    if (run.evidence)
      await run.page.screenshot({
        path: join(run.evidence, `handoff-${run.engine}-staged.png`),
        scale: "css",
      })
    await pickModel(run.page, picker, own)
    await unswitched(own)
    await pickModel(run.page, picker, target)
    await unswitched(target)
    if (failures.length) return
    const afterSwitch = prompt(run, STRESS.reply, step, " after switch")
    const seen = await send(
      run,
      conversationId,
      afterSwitch,
      failures,
      new Set(before.messages.map((each) => each.executionId)),
    )
    const switched = await waitFor(async () => {
      const view = await run.client.conversation.read(conversationId)
      return view.model === target.modelId &&
        view.handoffs.filter((each) => each.kind === "switch").length === 1 &&
        view.handoffs.some((each) => each.kind === "switch" && each.to?.agent === other)
        ? view
        : null
    }, 30_000)
    if (!switched) {
      failures.push(`sending did not switch the conversation to ${other} once`)
      return
    }
    if (
      !(await line
        .first()
        .waitFor({ timeout: 10_000 })
        .then(() => true)
        .catch(() => false))
    )
      failures.push("the window drew no line where the conversation was switched")
    const { turn: switchedTurn } = await settled(
      run,
      conversationId,
      seen,
      afterSwitch,
      "completed",
      failures,
    )
    carriedBy(switchedTurn, target.modelId, "the switch", first, failures)
    if (!(await carriedShown(run.page, target.modelId, afterSwitch)))
      failures.push("the window does not show the reply after the switch")
    if (run.evidence)
      await run.page.screenshot({
        path: join(run.evidence, `handoff-${run.engine}-switch.png`),
        scale: "css",
      })
    // Forked from the row: the window opens the fork, on the copied turns.
    const listed = new Set(
      (await run.client.conversation.list({})).conversations.map(
        (each) => each.conversationId,
      ),
    )
    await choose(run.page, conversationId, "Fork")
    const forkId = await waitFor(async () => {
      const { conversations } = await run.client.conversation.list({})
      return conversations.find((each) => !listed.has(each.conversationId))
        ?.conversationId
    }, 15_000)
    if (!forkId) {
      failures.push("the gateway never listed the fork")
      return
    }
    run.checked.push(forkId)
    const forked = await run.client.conversation.read(forkId)
    const copied = forked.handoffs.find((each) => each.kind === "fork")
    if (forked.model !== target.modelId)
      failures.push(`the fork runs ${forked.model}, not its source's ${target.modelId}`)
    if (!copied || (copied.turns?.length ?? 0) !== 2)
      failures.push(
        `the fork copied ${copied?.turns?.length ?? "no"} turns, not the source's 2`,
      )
    if (forked.messages.length > 0) failures.push("the fork said something of its own")
    if (
      !(await run.page
        .locator(`${css.handoffLine}[data-handoff="fork"]`)
        .first()
        .waitFor({ timeout: 10_000 })
        .then(() => true)
        .catch(() => false))
    )
      failures.push("the window did not open the fork on its copied turns")
    await expectShown(run.page, `${SAYS.echo}${first}`, failures)
    const inFork = prompt(run, STRESS.reply, step, " in fork")
    const forkSeen = await send(run, forkId, inFork, failures, new Set())
    const { turn: forkTurn } = await settled(
      run,
      forkId,
      forkSeen,
      inFork,
      "completed",
      failures,
    )
    // Its own first turn ran on the same model and was given the copied turns.
    carriedBy(forkTurn, target.modelId, "the fork", first, failures)
    if (!(await carriedShown(run.page, target.modelId, inFork)))
      failures.push("the window does not show the fork's own reply after the copied one")
    if (run.evidence)
      await run.page.screenshot({
        path: join(run.evidence, `handoff-${run.engine}-fork.png`),
        scale: "css",
      })
    // The source is as it was.
    const source = await run.client.conversation.read(conversationId)
    if (source.messages.length !== 2)
      failures.push(`the fork changed its source: ${source.messages.length} turns`)
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

  async trace(run, step, failures) {
    const text = prompt(run, STRESS.trace, step)
    const seen = await send(run, run.main.id, text, failures)
    const { view, turn } = await settled(
      run,
      run.main.id,
      seen,
      text,
      "completed",
      failures,
    )
    if (!turn) return
    // The gateway keeps every call of the turn with when it started and
    // ended, and the input it was made with.
    const tools = view.tools.filter((tool) => tool.executionId === turn.executionId)
    if (tools.length !== TRACE_CALLS.length) {
      failures.push(
        `the trace turn kept ${tools.length} calls, not ${TRACE_CALLS.length}`,
      )
      return
    }
    for (const [index, tool] of tools.entries()) {
      const call = TRACE_CALLS[index]
      const name = `call ${index + 1} (${call.tool})`
      if (tool.status !== call.status)
        failures.push(`${name} is ${tool.status}, not ${call.status}`)
      if (typeof tool.startedAtMs !== "number" || typeof tool.endedAtMs !== "number") {
        failures.push(
          `${name} kept no start or end: ${tool.startedAtMs}, ${tool.endedAtMs}`,
        )
        continue
      }
      if (tool.endedAtMs < tool.startedAtMs)
        failures.push(
          `${name} ended ${tool.startedAtMs - tool.endedAtMs} ms before it started`,
        )
      let input
      try {
        input = JSON.parse(tool.input)
      } catch {
        failures.push(`${name} kept no input: ${JSON.stringify(tool.input)}`)
        continue
      }
      // A harness reports the arguments themselves (Claude) or its own
      // record of the call, naming the tool (Codex): either way the input is
      // this call's, not another's.
      const own =
        input && typeof input === "object" && "arguments" in input && "tool" in input
          ? input.tool === call.tool && JSON.stringify(input.arguments)
          : JSON.stringify(input)
      if (own !== JSON.stringify(call.arguments))
        failures.push(
          `${name}'s input ${tool.input} is not ${JSON.stringify(call.arguments)}`,
        )
    }
    // The pause after each call shows in the times, which are the session's
    // own clock: the next call starts no sooner.
    for (let index = 1; index < tools.length; index += 1) {
      const gap = tools[index].startedAtMs - tools[index - 1].endedAtMs
      if (gap < TRACE_PAUSE_MS - 100)
        failures.push(
          `call ${index + 1} started ${gap} ms after the one before, not after the pause`,
        )
    }
    // The window draws the run as one line, folded once the agent wrote past
    // it, that opens to a row per call, each with its bar and status.
    const message = run.page
      .locator('.workspace-message[data-role="agent"]')
      .filter({ hasText: `${SAYS.trace}${text}` })
    const line = message.locator('[data-slot="tool-trace-line"]')
    const folded = await waitFor(
      async () =>
        (await line.count()) === 1 &&
        (await line.getAttribute("aria-expanded")) === "false" &&
        (await line.textContent()).includes(`Ran ${TRACE_CALLS.length} tools`),
      15_000,
    )
    if (run.evidence)
      await message.screenshot({
        path: join(run.evidence, `trace-${run.engine ?? "window"}-folded.png`),
      })
    if (!folded) {
      failures.push(
        `the window does not draw the run as one folded line: ${JSON.stringify(
          (await line.count()) ? await line.textContent() : null,
        )}`,
      )
      return
    }
    await line.click()
    const rows = message.locator('[data-slot="tool-trace-step"]')
    const opened = await waitFor(
      async () =>
        (await line.getAttribute("aria-expanded")) === "true" &&
        (await rows.count()) === TRACE_CALLS.length,
      5_000,
    )
    if (!opened) {
      failures.push(
        `the run opened to ${await rows.count()} rows, not ${TRACE_CALLS.length}`,
      )
      return
    }
    // Each bar starts where its call started on the shared axis, as a share
    // of the run: a settled run's axis spans the first call's start to the
    // last call's end (`ToolTrace`'s domain), so a clock read wrong — the
    // wrong sign, seconds for milliseconds — moves the bars.
    const drawn = await rows.evaluateAll((elements) =>
      elements.map((row) => {
        const bar = row.querySelector('[data-slot="tool-trace-bar"]')
        const track = bar?.parentElement?.getBoundingClientRect()
        const box = bar?.getBoundingClientRect()
        return {
          status: row.dataset.status,
          at: bar && track.width ? (box.left - track.left) / track.width : null,
          width: box?.width ?? 0,
          height: box?.height ?? 0,
          over: bar && track ? box.right - track.right : 0,
        }
      }),
    )
    const first = tools[0].startedAtMs
    const span = Math.max(1, ...tools.map((tool) => tool.endedAtMs - first))
    for (const [index, row] of drawn.entries()) {
      if (row.status !== TRACE_CALLS[index].status)
        failures.push(
          `row ${index + 1} is drawn ${row.status}, not ${TRACE_CALLS[index].status}`,
        )
      const expected = (tools[index].startedAtMs - first) / span
      if (row.at === null) failures.push(`row ${index + 1} has no bar on the time axis`)
      else if (Math.abs(row.at - expected) > 0.03)
        failures.push(
          `row ${index + 1}'s bar starts ${(row.at * 100).toFixed(1)}% along the axis, not ${(expected * 100).toFixed(1)}%`,
        )
      // An instant call is still a round dot, never a hairline, and the
      // last one stays inside the axis.
      if (row.at !== null && row.width + 0.5 < row.height)
        failures.push(
          `row ${index + 1}'s bar is ${row.width.toFixed(1)} px wide, narrower than its ${row.height.toFixed(1)} px height`,
        )
      if (row.over > 0.5)
        failures.push(
          `row ${index + 1}'s bar ends ${row.over.toFixed(1)} px past the axis`,
        )
    }
    // A call's card shows the input it was made with.
    const asked = TRACE_CALLS.findIndex((call) => Object.keys(call.arguments).length)
    const row = rows.nth(asked)
    await row.click()
    const card = message.locator(`[id="${await row.getAttribute("aria-controls")}"]`)
    await card.getByRole("tab", { name: "Input" }).click()
    const value = String(Object.values(TRACE_CALLS[asked].arguments)[0])
    const shown = await waitFor(
      async () =>
        (
          await card.locator('[role="tabpanel"][data-state="active"]').textContent()
        )?.includes(value),
      5_000,
    )
    if (!shown) failures.push(`call ${asked + 1}'s card does not show its input ${value}`)
    if (run.evidence)
      await message.screenshot({
        path: join(run.evidence, `trace-${run.engine ?? "window"}-opened.png`),
      })
    // Folded again, as the person left it.
    await line.click()
    if (
      !(await waitFor(
        async () => (await line.getAttribute("aria-expanded")) === "false",
        5_000,
      ))
    )
      failures.push("the run does not fold again")
  },

  async table(run, step, failures) {
    const text = prompt(run, STRESS.table, step)
    const seen = await send(run, run.main.id, text, failures)
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    if (!turn) return
    // The gateway keeps the answer as it streamed: every line, in order, once.
    const said = (turn.parts ?? []).map((part) => part.text ?? "").join("")
    const expected = `${SAYS.table}${text}\n\n${TABLE_LINES.map((line) => `${line}\n`).join("")}`
    if (said.trimEnd() !== expected.trimEnd())
      failures.push(`the table answer came back as ${JSON.stringify(said.slice(0, 120))}`)
    // The window draws both tables in the agent's message, and draws them
    // whole: no word split across lines, the narrow table inside the reply,
    // the wide one scrolling in its own panel with its hidden side faded.
    const message = run.page
      .locator('.workspace-message[data-role="agent"]')
      .filter({ hasText: `${SAYS.table}${text}` })
    const tables = message.locator('[data-slot="markdown-table"]')
    // Both tables whole: the last cell streamed is drawn, and the window has
    // finished revealing the answer (no word still in its fade-in span), so
    // every column has its final width.
    const lastCell = TABLE_LINES.at(-1).split("|").at(-2).trim()
    const drawn = await waitFor(
      async () =>
        (await tables.count()) === 2 &&
        (await tables.nth(1).getByText(lastCell, { exact: true }).count()) === 1 &&
        (await message.locator('span[class*="starting:"]').count()) === 0,
      15_000,
    )
    if (!drawn) {
      failures.push(
        `the window drew ${await tables.count()} of 2 tables, or not all of them`,
      )
      return
    }
    // Short runs each table holds, which a squeezed column used to split.
    const words = [
      ["Price", "$16.99", "CAD", "$150"],
      ["Runtime", "Format", "Projectionist"],
    ]
    const measured = await message.evaluate((element, words) => {
      const lines = (root, word) => {
        const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
        for (let node = walker.nextNode(); node; node = walker.nextNode()) {
          const at = node.textContent.indexOf(word)
          if (at < 0) continue
          const range = document.createRange()
          range.setStart(node, at)
          range.setEnd(node, at + word.length)
          return new Set([...range.getClientRects()].map((rect) => Math.round(rect.top)))
            .size
        }
        return 0
      }
      const reply = element.getBoundingClientRect()
      return [...element.querySelectorAll('[data-slot="markdown-table"]')].map(
        (table, index) => {
          const port = table.querySelector('[data-slot="table-container"]')
          return {
            rows: table.querySelectorAll("tbody tr").length,
            missing: words[index].filter((word) => lines(table, word) === 0),
            split: words[index].filter((word) => lines(table, word) > 1),
            hidden: port.scrollWidth - port.clientWidth,
            within: table.getBoundingClientRect().right <= reply.right + 1,
            fadedRight: port.hasAttribute("data-hidden-right"),
          }
        },
      )
    }, words)
    // Each table shows its data rows once, as streamed: a table's lines less
    // its heading and delimiter.
    const rows = TABLE_LINES.join("\n")
      .split("\n\n")
      .map((table) => table.split("\n").length - 2)
    for (const [index, table] of measured.entries()) {
      if (table.rows !== rows[index])
        failures.push(`table ${index + 1} shows ${table.rows} rows, not ${rows[index]}`)
      if (table.missing.length)
        failures.push(`table ${index + 1} does not show ${JSON.stringify(table.missing)}`)
      if (table.split.length)
        failures.push(
          `table ${index + 1} split ${JSON.stringify(table.split)} across lines`,
        )
      if (!table.within) failures.push(`table ${index + 1} is wider than the reply`)
    }
    const [narrow, wide] = measured
    if (narrow.hidden > 1 || narrow.fadedRight)
      failures.push(`the narrow table scrolls (${narrow.hidden} px hidden)`)
    if (wide.hidden <= 1)
      failures.push(
        `the wide table does not scroll in its panel (${wide.hidden} px hidden)`,
      )
    const port = tables.nth(1).locator('[data-slot="table-container"]')
    const faded = (left, right) =>
      waitFor(
        async () =>
          ((await port.getAttribute("data-hidden-left")) !== null) === left &&
          ((await port.getAttribute("data-hidden-right")) !== null) === right,
        5_000,
      )
    if (!(await faded(false, true)))
      failures.push(
        "the wide table does not fade its right side, where columns are hidden",
      )
    if (run.evidence)
      await run.page.screenshot({
        path: join(run.evidence, `table-${run.engine ?? "window"}.png`),
      })
    // Scrolled to its end, the fade moves to the side now hidden.
    await port.evaluate((each) => each.scrollTo({ left: each.scrollWidth }))
    if (!(await faded(true, false)))
      failures.push("scrolled to its end, the wide table did not fade its left side")
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
 * Two panes side by side are regions of one flat chat surface, not cards
 * (`flatSurfaceFailures`): one hairline between them, mid-gutter.
 */
/**
 * The window shows a thread's report as one line — the thread's chat as a
 * pill with its title, and what it did — never the report's text or ids;
 * pressed, the pill brings that chat to the front, in the focused pane.
 */
async function reportLine(run, report, failures) {
  const thread = /^\[Thread report\] Thread (\S+) /.exec(report.userText)?.[1]
  if (!thread) {
    failures.push(`the report did not name its thread: ${report.userText.slice(0, 120)}`)
    return
  }
  const line = run.page.locator('[data-role="thread-report"]').filter({
    has: run.page.locator(`[data-chat-pill="${thread}"]`),
  })
  const shown = await line
    .first()
    .waitFor({ timeout: 15_000 })
    .then(() => true)
    .catch(() => false)
  if (!shown) {
    failures.push("the thread's report is not one line with the thread's pill")
    return
  }
  const title = await titleOf(run, thread)
  const said = (await line.first().innerText()).replace(/\s+/g, " ").trim()
  if (!said.includes(title) || !said.endsWith("responded"))
    failures.push(`the report line reads "${said}", not "${title} responded"`)
  if (
    /finished the turn you sent it|execution /.test(
      await run.page.locator(css.pane).first().innerText(),
    )
  )
    failures.push("the window shows the report's text, not just its line")
  await line.first().locator(`[data-chat-pill="${thread}"]`).click()
  // In front: the focused pane shows it, which its list row says.
  const front = await waitFor(
    async () =>
      (await selectedStressConversation(run.page, await titleOf(run, thread))) === thread,
    10_000,
  )
  if (!front)
    failures.push("pressing the report's pill did not bring its chat to the front")
  await openStressConversation(run.page, run.main.id, run.main.title)
}

/**
 * `oneHeaderPerChat` once the panes come to rest after a column folds or
 * opens: what it measures is read again until it holds, or the columns'
 * motion has had its time; what still fails then is the step's.
 */
async function headersLand(page, when, failures) {
  let last = []
  await waitFor(async () => {
    last = []
    await oneHeaderPerChat(page, when, last)
    return last.length === 0
  }, 5_000)
  failures.push(...last)
}

/**
 * The chats have no tabs (C1): no tab strip, and each pane has one header —
 * its name, menu and close — in the titlebar's row, none of it on the
 * window's controls (the cluster on the left, the panel's own on the
 * right), and its picture up to the window's top edge, behind them.
 */
async function oneHeaderPerChat(page, when, failures) {
  const state = await page.evaluate(
    ({ titlebar, pane, header }) => {
      const box = (e) => {
        const b = e.getBoundingClientRect()
        return { left: b.left, top: b.top, right: b.right, bottom: b.bottom }
      }
      const shown = (e) => e.getClientRects().length > 0
      return {
        // Any tab list outside the right panel would be tabs for the chats.
        strip: [...document.querySelectorAll('[role="tablist"]')].filter(
          (list) => !list.closest("#workspace-panel"),
        ).length,
        bar: document.querySelector(titlebar).getBoundingClientRect().height,
        controls: [
          ...document.querySelectorAll(
            `${titlebar} .workspace-titlebar-navigation button, ${titlebar} .workspace-titlebar-trailing button`,
          ),
        ]
          .filter(shown)
          .map(box),
        panes: [...document.querySelectorAll(pane)].map((each) => ({
          headers: [...each.querySelectorAll(header)].map((h) => ({
            box: box(h),
            parts: [...h.querySelectorAll("button, .workspace-pane-name")]
              .filter(shown)
              .map(box),
          })),
          box: box(each),
          picture: [...each.querySelectorAll(".desktop-header, .desktop-header-image")]
            .filter(shown)
            .map(box)[0],
        })),
      }
    },
    { titlebar: css.titlebar, pane: css.pane, header: css.paneHeader },
  )
  const overlaps = (a, b) =>
    a.left < b.right - 1 &&
    b.left < a.right - 1 &&
    a.top < b.bottom - 1 &&
    b.top < a.bottom - 1
  if (state.strip) failures.push(`${when}: a tab strip is drawn for the chats`)
  for (const [at, pane] of state.panes.entries()) {
    const name = `${when}, pane ${at + 1}`
    if (pane.headers.length !== 1) {
      failures.push(`${name}: ${pane.headers.length} headers, not one`)
      continue
    }
    const [header] = pane.headers
    if (header.box.top < -1 || header.box.bottom > state.bar + 1)
      failures.push(
        `${name}: its header is not in the titlebar row: ${JSON.stringify(header.box)}`,
      )
    for (const part of header.parts)
      for (const control of state.controls)
        if (overlaps(part, control))
          failures.push(
            `${name}: its header sits on a window control: ${JSON.stringify({ part, control })}`,
          )
    if (
      pane.picture &&
      (pane.picture.top > 0.5 || pane.picture.left > pane.box.left + 0.5)
    )
      failures.push(
        `${name}: its picture does not reach the window's top and its pane's left: ${JSON.stringify({ picture: pane.picture, pane: pane.box })}`,
      )
  }
  return state
}

async function flatPanes(page, failures) {
  const now = async () => ({
    panes: await page.locator(css.pane).count(),
    hairlines: await page.locator(`${css.chat} ${css.paneEdge}[data-axis="x"]`).count(),
    broken: await flatSurfaceFailures(page),
  })
  const holds = (state) =>
    state.panes === 2 && state.hairlines === 1 && state.broken.length === 0
  if (await waitFor(async () => holds(await now()), 10_000)) return
  failures.push(`the panes are not one flat surface: ${JSON.stringify(await now())}`)
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

/** Opens Settings › Connections › Linked devices, as a person does. */
async function openLinkedDevices(page) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await page.locator(css.settingsCategory, { hasText: names.connections }).click()
  await page.locator(css.settingsTab, { hasText: names.linkedDevices }).click()
}

/**
 * Asks for setup again where a person does: Settings › Advanced › Setup, its
 * Replay button. Resolves how many times the host was asked. Leaves Settings.
 */
async function replayOnboarding(page, evidence) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await page.locator(css.settingsCategory, { hasText: names.advanced }).click()
  await page.locator(css.settingsTab, { hasText: names.setupTab }).click()
  if (evidence)
    await page
      .screenshot({ path: join(evidence, "onboarding-settings.png") })
      .catch(() => {})
  const before = await page.evaluate(() => window.__fakeHostReplays)
  await page
    .locator(selectorFor.settingRow("replay-onboarding"))
    .getByRole("button", { name: names.replayOnboarding, exact: true })
    .click()
  await waitFor(
    async () => (await page.evaluate(() => window.__fakeHostReplays)) > before,
    5_000,
  )
  const asked = (await page.evaluate(() => window.__fakeHostReplays)) - before
  await closeSettings(page)
  return asked
}

/** The menu-bar panel as the host opens it, over the run's gateway. */
async function panelWindow(run) {
  const page = await run.page.context().newPage()
  await page.setViewportSize({ width: 420, height: 760 })
  await page.addInitScript(gatewayHost, {
    endpoint: run.stack.endpoint,
    credential: run.stack.credential,
    panel: true,
  })
  await page.goto(`${new URL(run.page.url()).origin}/index.html`)
  return page
}

/** Waits until the panel's strip leads with `title`, the selected tab's name. */
async function panelNamed(page, title, failures) {
  const named = await waitFor(
    async () =>
      (
        await page
          .locator(css.panelCurrent)
          .innerText()
          .catch(() => "")
      ).includes(title),
    15_000,
  )
  if (!named)
    failures.push(`the panel's strip does not lead with ${JSON.stringify(title)}`)
}

/**
 * A setup window as the host opens it, over the run's gateway, recording
 * each beat of its opening as it comes: the beat, and whether the window's
 * heading could be seen by then.
 */
async function setupWindow(run, prepare) {
  const page = await run.page.context().newPage()
  // Before the page loads, so nothing it asks on its first frames gets past.
  await prepare?.(page)
  await page.addInitScript(gatewayHost, {
    endpoint: run.stack.endpoint,
    credential: run.stack.credential,
    setup: true,
  })
  await page.addInitScript(() => {
    window.__setupBeats = []
    window.__setupLeft = false
    new MutationObserver(() => {
      const shell = document.querySelector(".nessa-setup-window")
      if (shell?.hasAttribute("data-leaving")) window.__setupLeft = true
      const beat = shell?.dataset.beat
      if (!beat || window.__setupBeats.at(-1)?.beat === beat) return
      // Seen as a person sees it: a pane held hidden or clear until the
      // window opens is in the page from the first frame.
      const heading = document.getElementById("nessa-setup-heading")
      const headingSeen = Boolean(
        heading?.checkVisibility({ checkVisibilityCSS: true, checkOpacity: true }),
      )
      // Where the window is drawn as the beat starts, transform included.
      const { left, top, right, bottom } = shell.getBoundingClientRect()
      const drawn = { left, top, right, bottom, width: innerWidth, height: innerHeight }
      window.__setupBeats.push({ beat, headingSeen, drawn })
    }).observe(document, { subtree: true, childList: true, attributes: true })
    // The flight is the window's one Web Animation: the box it took off
    // from and the keyframes and timing it was given, taken as it starts.
    window.__setupFlights = []
    const animate = Element.prototype.animate
    Element.prototype.animate = function (...args) {
      const animation = animate.apply(this, args)
      if (this.classList.contains("nessa-setup-window")) {
        const { left, top, width, height } = this.getBoundingClientRect()
        window.__setupFlights.push({
          from: { left, top, width, height },
          frames: animation.effect.getKeyframes(),
          duration: animation.effect.getComputedTiming().duration,
        })
      }
      return animation
    }
  })
  await page.goto(`${new URL(run.page.url()).origin}/index.html?surface=setup`)
  return page
}

/**
 * The opening was a glow on the desktop that became the window: nothing
 * dimmed what is behind setup, the beats came in order, and none of the
 * window's words showed before the window had formed.
 */
async function openedAsAGlow(page, failures) {
  const beats = await page.evaluate(() => window.__setupBeats)
  const order = beats.map((entry) => entry.beat).join(",")
  if (order !== "glow,fill,form,open")
    failures.push(`setup opened as ${order}, not glow,fill,form,open`)
  const early = beats.filter((entry) => entry.beat !== "open" && entry.headingSeen)
  if (early.length)
    failures.push(`the window's heading showed during the ${early[0].beat} beat`)
  // Glow and fill draw the window past every edge of the screen.
  for (const { beat, drawn } of beats.filter(
    (entry) => entry.beat !== "open" && entry.beat !== "form",
  ))
    if (
      drawn.left > 0 ||
      drawn.top > 0 ||
      drawn.right < drawn.width ||
      drawn.bottom < drawn.height
    )
      failures.push(
        `during ${beat} the light does not fill the screen: ${JSON.stringify(drawn)}`,
      )
  // Once open, the window has formed: its own size, in the middle.
  const formed = () =>
    page.locator(".nessa-setup-window").evaluate((shell) => {
      const box = shell.getBoundingClientRect()
      return {
        scale: box.width / shell.offsetWidth,
        dx: box.left + box.width / 2 - innerWidth / 2,
        dy: box.top + box.height / 2 - innerHeight / 2,
      }
    })
  const whole = (shape) =>
    Math.abs(shape.scale - 1) < 0.005 && Math.abs(shape.dx) < 1 && Math.abs(shape.dy) < 1
  if (!(await waitFor(async () => whole(await formed()), 3_000)))
    failures.push(
      `the open window did not form in the middle: ${JSON.stringify(await formed())}`,
    )
  const backdrop = await page.evaluate(() => ({
    dim: document.querySelectorAll(".nessa-setup-dim").length,
    sheet: getComputedStyle(document.querySelector(".nessa-setup-sheet")).backgroundColor,
  }))
  if (backdrop.dim || backdrop.sheet !== "rgba(0, 0, 0, 0)")
    failures.push(`setup dims the desktop: ${JSON.stringify(backdrop)}`)
}

/**
 * The picker lists Claude, Codex and a Local AI that cannot be chosen yet,
 * and never OpenCode. The picture wears one agent's background with that
 * agent's figure on it, the chosen agent's (Claude Code's mascot for Claude),
 * and pointing at a row turns it over to that agent.
 */
async function pickerOffers(page, failures) {
  const rows = await page
    .getByRole("group", { name: "Agent" })
    .locator(AGENT_ROWS)
    .evaluateAll((buttons) =>
      buttons.map((button) => ({
        name: button.getAttribute("aria-label") ?? button.textContent,
        disabled: button.getAttribute("aria-disabled") === "true",
      })),
    )
  const named = rows.map((row) => row.name)
  for (const agent of names.setup.agents)
    if (!named.some((name) => name === agent || name.startsWith(`${agent},`)))
      failures.push(`the picker does not list ${agent}: ${JSON.stringify(named)}`)
  const local = rows.find((row) => row.name === names.setup.localAi)
  if (!local?.disabled)
    failures.push(`Local AI is not listed as coming: ${JSON.stringify(rows)}`)
  if (named.some((name) => /opencode/i.test(name)))
    failures.push(`the picker still offers OpenCode: ${JSON.stringify(named)}`)
  // What the picture shows: the opaque background, then every figure on it.
  const cards = () =>
    page
      .locator(".nessa-setup-art")
      .evaluate((art) =>
        [
          art.querySelector("[data-slot='agent-backdrop'][data-shown]")?.dataset.agent,
          ...[...art.querySelectorAll("[data-slot='agent-figure']")].map(
            (figure) => figure.dataset.agent,
          ),
        ].join(","),
      )
  const resting = await cards()
  if (resting !== "clawd,clawd")
    failures.push(`the picture shows ${JSON.stringify(resting)}`)
  // Pointing along the rows leaves the picture as it is: only a click
  // changes it. Watched for a while, since a change would be a crossfade.
  const codex = page.getByRole("button", { name: "Codex", exact: true })
  await codex.hover()
  const moved = await waitFor(
    () => cards().then((shown) => shown !== "clawd,clawd"),
    1_000,
  )
  if (moved)
    failures.push(
      `pointing at Codex turned the picture to ${JSON.stringify(await cards())}`,
    )
  await codex.click()
  const chosen = await waitFor(
    () => cards().then((shown) => shown === "codex,codex"),
    5_000,
  )
  if (!chosen)
    failures.push(
      `clicking Codex did not turn the picture to it: ${JSON.stringify(await cards())}`,
    )
  await page.getByRole("button", { name: "Claude", exact: true }).hover()
  if (await waitFor(() => cards().then((shown) => shown !== "codex,codex"), 1_000))
    failures.push(
      `pointing at Claude after choosing Codex turned the picture to ${JSON.stringify(await cards())}`,
    )
  await page.mouse.move(1, 1)
}

/**
 * The gateway's machine probe, answered here: this stack's page is not an
 * origin it answers. `readiness` is read on every ask, so a step can change
 * what the machine has between asks.
 */
function agentsProbe(page, readiness) {
  return page.route("**/onboarding/agents", (route) =>
    route.fulfill({
      // The gateway's own origin, not the page's: the ask is cross-origin.
      headers: { "access-control-allow-origin": "*" },
      json: {
        agents: Object.entries(readiness()).map(([id, state]) => ({
          id,
          readiness: state,
        })),
      },
    }),
  )
}

/** The size the fake installer gives Codex's download. */
const CODEX_ARCHIVE_BYTES = 45_000_000

/**
 * A gateway whose installer offers Codex and installs it on request, without
 * fetching anything: the page's socket goes to the real gateway, and only
 * the installer's two asks are answered here. `installed` says when the
 * install has been asked for, so the machine probe can follow it.
 */
async function codexInstaller(page) {
  const state = { installed: false, installs: 0 }
  await page.routeWebSocket(
    () => true,
    (socket) => {
      const server = socket.connectToServer()
      const answer = (id, payload) => {
        try {
          socket.send(JSON.stringify({ type: "res", id, ok: true, payload }))
        } catch {
          // The window closed first: nothing is waiting for the answer.
        }
      }
      socket.onMessage((message) => {
        let frame
        try {
          frame = typeof message === "string" ? JSON.parse(message) : undefined
        } catch {
          // Not a request this fake answers: it goes to the gateway as sent.
        }
        if (frame?.type !== "req") frame = undefined
        if (frame?.method === "agents.installOptions")
          return answer(frame.id, {
            agents: [
              {
                agent: "codex",
                version: "0.0.0-stress",
                archiveBytes: CODEX_ARCHIVE_BYTES,
                installed: state.installed,
              },
            ],
          })
        if (frame?.method === "agents.install" && frame.params?.agent === "codex") {
          state.installs += 1
          // Long enough that the card is seen fetching.
          return void setTimeout(() => {
            state.installed = true
            answer(frame.id, {
              agent: "codex",
              version: "0.0.0-stress",
              downloaded: true,
              cleanupPending: false,
            })
          }, 600)
        }
        server.send(message)
      })
      server.onMessage((message) => socket.send(message))
    },
  )
  return state
}

/**
 * An agent that is not installed but can be downloaded is a card with one
 * Download button where its row would be, and nothing else on the picker
 * talks about downloads. Its Download installs it, and the card becomes the
 * agent's row.
 */
async function downloadCard(page, installer, evidence, failures) {
  const group = page.getByRole("group", { name: "Agent" })
  const card = group.getByRole("group", { name: names.setup.codexCard })
  const download = card.getByRole("button", {
    name: names.setup.codexDownload,
    exact: true,
  })
  const shown = await waitFor(async () => (await download.count()) === 1, 15_000)
  await page.screenshot({ path: join(evidence, "onboarding-download-card.png") })
  if (!shown) {
    failures.push("Codex, not installed but offered, is not shown as a download card")
    return
  }
  if (await group.locator("button[aria-pressed]", { hasText: "Codex" }).count())
    failures.push("Codex has both a download card and a row to pick")
  for (const text of [/Check downloads/, /Download only the agents/, /^Installed$/])
    if (await page.getByText(text).count()) failures.push(`the picker still says ${text}`)
  // Pressed from the keyboard, so where the focus goes is part of the check.
  await download.focus()
  await page.keyboard.press("Enter")
  // The installer holds its answer, so the card is seen fetching, with the
  // focus still on its button.
  const fetching = await waitFor(
    () =>
      card.evaluate(
        (each) =>
          each.dataset.downloadState === "fetching" &&
          each.contains(document.activeElement),
      ),
    2_000,
  )
  if (!fetching) failures.push("Codex's card did not fetch with the focus kept on it")
  const row = group.locator("button[aria-pressed]:not([aria-disabled='true'])", {
    hasText: "Codex",
  })
  if (!(await waitFor(async () => (await row.count()) === 1, 15_000)))
    failures.push("the installed Codex did not become a row to pick")
  else {
    if (await card.count()) failures.push("Codex's download card stayed after install")
    // The focus the download held is handed to the row it became.
    if (
      !(await waitFor(
        () => row.evaluate((each) => each === document.activeElement),
        2_000,
      ))
    )
      failures.push("the focus was not handed to Codex's row after install")
  }
  if (installer.installs !== 1)
    failures.push(`Download asked the installer ${installer.installs} times, not once`)
}

/** The picker's agent rows, and not the buttons of a key form beside one. */
const AGENT_ROWS = `button[aria-pressed], button[aria-label='${names.setup.localAi}']`

/**
 * Setup finished on a ready agent leaves by flying into the menu bar: the
 * window lets go of its pane, the host is asked where its icon is, and the
 * handoff that follows is a finish on that agent, made once the window has
 * landed and gone.
 *
 * Which agents are ready is the gateway's machine probe, which this stack's
 * page is not an origin it answers; the probe is answered here instead with
 * Claude ready, so the step is about the finish rather than about the machine.
 * Codex starts not installed, with a download on offer, so the same window
 * shows its download card on the way.
 */
async function finishedIntoTheMenuBar(run, failures) {
  let installer
  const page = await setupWindow(run, async (page) => {
    installer = await codexInstaller(page)
    await agentsProbe(page, () => ({
      claude: "ready",
      codex: installer.installed ? "ready" : "not-installed",
    }))
  })
  try {
    const button = (name) => page.getByRole("button", { name, exact: true })
    await page
      .getByRole("heading", { name: names.setup.welcome, exact: true })
      .waitFor({ timeout: 15_000 })
    await button(names.setup.getStarted).click()
    await downloadCard(page, installer, run.evidence, failures)
    const ready = page
      .getByRole("group", { name: "Agent" })
      .locator("button[aria-pressed]:not([aria-disabled='true'])")
    if (!(await waitFor(async () => (await ready.count()) > 0, 15_000))) {
      const rows = await page
        .getByRole("group", { name: "Agent" })
        .locator(AGENT_ROWS)
        .evaluateAll((all) => all.map((row) => row.getAttribute("aria-label")))
      failures.push(`no agent became ready to choose in setup: ${JSON.stringify(rows)}`)
      return
    }
    const chosen = (await ready.first().getAttribute("aria-label"))?.toLowerCase()
    await ready.first().click()
    await button(names.setup.continue).click()
    const finish = page.getByRole("button", {
      name: new RegExp(`^(${names.setup.finish.join("|")})$`),
    })
    await finish.waitFor({ timeout: 10_000 })
    await finish.click()
    const handed = await waitFor(
      async () => (await page.evaluate(() => window.__fakeHostSetupFinishes.length)) > 0,
      10_000,
    )
    const { finishes, trayAsks, left, flights, tray } = await page.evaluate(() => ({
      finishes: window.__fakeHostSetupFinishes,
      trayAsks: window.__fakeHostTrayAsks,
      left: window.__setupLeft,
      flights: window.__setupFlights,
      // Where the fake host says its icon is.
      tray: { x: innerWidth - 80, y: 12 },
    }))
    if (!left) failures.push("finishing setup did not fly the window")
    // Where the flight lands, measured from what it was given rather than
    // from its frames: a loaded headless page draws few frames, and the
    // flight's deadline can land setup before the last one. A box where the
    // window took off, wearing the last keyframe, is drawn where the window
    // ends up; its centre is on the icon, and it is a light the icon's size.
    // How long it takes on screen in a production build is measured on a Mac.
    if (flights.length !== 1)
      failures.push(`the window flew ${flights.length} times, not once`)
    else {
      const [flight] = flights
      const landing = await page.evaluate(({ from, frames }) => {
        const last = frames.at(-1)
        const probe = document.createElement("div")
        Object.assign(probe.style, {
          position: "fixed",
          left: `${from.left}px`,
          top: `${from.top}px`,
          width: `${from.width}px`,
          height: `${from.height}px`,
          transform: last.transform,
        })
        document.body.append(probe)
        const box = probe.getBoundingClientRect()
        probe.remove()
        return {
          x: box.left + box.width / 2,
          y: box.top + box.height / 2,
          width: box.width,
          height: box.height,
          opacity: Number(last.opacity),
          round: frames.slice(1).every((frame) => frame.borderRadius === "50%"),
          offset: last.offset,
        }
      }, flight)
      if (
        Math.abs(landing.x - tray.x) > 1 ||
        Math.abs(landing.y - tray.y) > 1 ||
        Math.abs(landing.width - 17.6) > 0.5 ||
        Math.abs(landing.height - 17.6) > 0.5 ||
        landing.opacity !== 0 ||
        !landing.round ||
        flight.duration !== 900
      )
        failures.push(
          `the flight lands ${JSON.stringify({ ...landing, duration: flight.duration })}, not a round 17.6 px light on ${JSON.stringify(tray)} over 900 ms`,
        )
    }
    if (!handed) failures.push("finishing setup did not hand over to the host")
    else if (
      finishes.length !== 1 ||
      finishes[0].completed !== true ||
      finishes[0].agent !== chosen
    )
      failures.push(
        `finishing handed over ${JSON.stringify(finishes)}, not one finish on ${chosen}`,
      )
    else if (finishes[0].windowShown)
      failures.push("setup handed over while its window was still on screen")
    if (trayAsks !== 1)
      failures.push(
        `the flight asked where the menu bar icon is ${trayAsks} times, not once`,
      )
  } finally {
    await page.close()
  }
}

/**
 * Where setup's corner buttons are, against its window: which buttons are
 * drawn, and for each its centre as fractions of the window's width and
 * height, and whether it is round.
 */
function setupCorners(page) {
  return page.evaluate((labels) => {
    const frame = document.querySelector(".nessa-setup-window")?.getBoundingClientRect()
    const at = (label) => {
      const button = document.querySelector(`button[aria-label="${label}"]`)
      if (!frame || !button) return null
      const box = button.getBoundingClientRect()
      return {
        x: (box.left + box.width / 2 - frame.left) / frame.width,
        y: (box.top + box.height / 2 - frame.top) / frame.height,
        round:
          Math.abs(box.width - box.height) < 0.5 &&
          parseFloat(getComputedStyle(button).borderRadius) >= box.width / 2 - 0.5,
      }
    }
    return { back: at(labels.back), close: at(labels.close) }
  }, names.setup)
}

/** Whether a corner button sits in the window's top-left or top-right corner. */
function inCorner(place, side) {
  if (!place || !place.round || place.y > 0.15) return false
  return side === "left" ? place.x < 0.15 : place.x > 0.85
}

/**
 * Nessa's own shell tool as the gateway's managed `nessa` server: the
 * `nessa-mcp` built beside the gateway, working in a directory of the run's.
 */
function shellServer(directory) {
  const gateway = process.env.MCP_LIVE_NESSA ?? join(repoRoot, "target/debug/nessa")
  const command = join(dirname(gateway), "nessa-mcp")
  if (!existsSync(command))
    throw new CannotRun(
      `build the shell tool first (cargo build -p nessa-mcp): ${command}`,
    )
  const workspace = join(directory, "shell-workspace")
  mkdirSync(workspace)
  return {
    name: "nessa",
    command,
    args: ["--workspace", workspace, "--audit-directory", join(directory, "shell-audit")],
  }
}

/** Opens Settings › Connections › Accounts, where the GitHub row is. */
async function openAccounts(page) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await page.locator(css.settingsCategory, { hasText: names.connections }).click()
  await page.locator(css.settingsTab, { hasText: names.accounts }).click()
}

/**
 * A new conversation's agent, opened now, says what GitHub sign-in it was
 * started with; `said` is what it must say. A sign-in is read when an agent
 * opens, so only a new one shows a change.
 */
async function githubAgent(run, step, extra, said, failures, shell = false) {
  const conversationId = randomUUID()
  run.checked.push(conversationId)
  const text = prompt(run, shell ? STRESS.githubShell : STRESS.github, step, extra)
  await run.client.conversation.create({ conversationId, agent: run.stack.agent })
  await run.client.conversation.submit(
    run.client.conversation.prepare(conversationId, text),
  )
  const { turn } = await settled(
    run,
    conversationId,
    new Set(),
    text,
    "completed",
    failures,
  )
  if (!turn) return
  const answer = (turn.parts ?? []).map((part) => part.text ?? "").join("")
  const where = shell ? SAYS.githubShell : SAYS.github
  if (answer !== `${where}${said}`)
    failures.push(
      `an agent started${extra} says ${JSON.stringify(answer)}${shell ? " from its shell tool" : ""}, not ${said}`,
    )
}

/** Leaves Settings: Esc does not close it; its back control returns to the app. */
async function closeSettings(page) {
  await page.getByRole("button", { name: "Back to nessa Agent" }).first().click()
  await page.waitForSelector(css.settings, { state: "detached", timeout: 10_000 })
}

/**
 * With linking on, the gateway is found on the local network the way a phone
 * that scanned the code finds it: a DNS-SD browse for `_nessa-link._tcp`
 * hears it on its native port, with its gateway id. Resolves whether it did.
 */
async function announced(run, failures) {
  const port = Number(run.nativeListen.split(":").pop())
  let heard = []
  for (
    let attempt = 0;
    attempt < 3 && !heard.some((each) => each.port === port);
    attempt++
  )
    heard = await browse("_nessa-link._tcp.local", 3_000)
  const gateway = heard.find((each) => each.port === port)
  if (!gateway) {
    failures.push(
      `the gateway was not announced on port ${port}; heard ${JSON.stringify(heard)}`,
    )
    return false
  }
  if (!gateway.txt.gw) failures.push("the announcement carries no gateway id")
  return Boolean(gateway.txt.gw)
}

/**
 * Pair a device on the Linked devices tab, with linking on: the window's own
 * sign-in opens a code, shown as a person pairing a phone sees it, never the
 * card that says this sign-in cannot manage credentials. The code opens in a
 * modal with the one signal orb, none of it drawn on the page; Escape hides
 * the modal and leaves the code open, and Show code brings back the same
 * code. Cancel in the modal then closes it and Pair a device is offered
 * again. Resolves whether all of that held.
 */
async function pairFromWindow(run, failures) {
  const { page } = run
  const before = failures.length
  const dialog = page.locator(css.pairingDialog)
  const codeIn = async () =>
    (await dialog.locator('[data-slot="pairing-code-value"]').innerText()).replace(
      /\s+/g,
      "",
    )
  await page.locator(selectorFor.linkedAction("pair")).click()
  const shown = await waitFor(
    async () =>
      (await page.locator(css.linkedRefused).count()) > 0
        ? "refused"
        : (await dialog.locator(css.pairingCode).count()) > 0
          ? "code"
          : null,
    10_000,
  )
  if (shown !== "code") {
    failures.push(
      shown === "refused"
        ? "Pair a device was refused: the window's sign-in cannot manage credentials"
        : "Pair a device opened no modal with a code",
    )
    return false
  }
  // The orb gathers in, then stays live: animated, not drawn still.
  const orb = dialog.locator(css.signalOrb)
  const live = await waitFor(
    async () => (await orb.getAttribute("data-state").catch(() => null)) === "live",
    5_000,
  )
  if (!live)
    failures.push(
      `the pairing orb was ${await orb.getAttribute("data-state").catch(() => "missing")}, not live`,
    )
  if (run.evidence)
    await page
      .screenshot({ path: join(run.evidence, `pairing-${run.engine}-modal.png`) })
      .catch(() => {})
  const code = await codeIn()
  if (!code) failures.push("the pairing code shown was empty")
  // Settings is a fixed layer over the window: a modal portalled past it is
  // in the page but drawn underneath it, unseen.
  if (!(await dialog.evaluate((node) => Boolean(node.closest("[data-settings-layers]")))))
    failures.push("the pairing modal is not inside Settings, so it is drawn under it")
  const orbs = await dialog.locator(css.signalOrb).count()
  if (orbs !== 1) failures.push(`the pairing modal drew ${orbs} signal orbs, not 1`)
  if ((await page.locator(css.qrOrb).count()) > 0) failures.push("a QR orb was drawn")
  if ((await page.locator(`${css.linked} ${css.pairingCode}`).count()) > 0)
    failures.push("the pairing code was drawn on the tab, not only in the modal")
  await page.keyboard.press("Escape")
  await dialog
    .waitFor({ state: "detached", timeout: 10_000 })
    .catch(() => failures.push("Escape did not close the pairing modal"))
  await page.locator(selectorFor.linkedAction("show")).click()
  await dialog
    .waitFor({ timeout: 10_000 })
    .catch(() => failures.push("Show code did not open the pairing modal again"))
  const again = await codeIn().catch(() => "")
  if (again !== code)
    failures.push(`Show code showed ${JSON.stringify(again)}, not the open code`)
  await dialog.locator(selectorFor.linkedAction("cancel")).click()
  await dialog
    .waitFor({ state: "detached", timeout: 10_000 })
    .catch(() => failures.push("the cancelled code's modal stayed open"))
  await page
    .locator(selectorFor.linkedAction("pair"))
    .waitFor({ timeout: 10_000 })
    .catch(() => failures.push("Pair a device was not offered again after cancelling"))
  return failures.length === before
}

/**
 * Turns native linking `enabled` with the switch on the Linked devices tab,
 * already open. The gateway saves config.json and starts itself again in
 * the same process; the window comes back to the new state, and the run's
 * client is connected again. Resolves whether all of that held. Called with
 * nothing else in flight (`run.exclusive`).
 */
async function switchLinking(run, enabled, failures) {
  const gateway = run.stack.gateway
  const config = join(gateway.directory, "ci", "instances", INSTANCE, "config.json")
  const pid = gateway.server.pid
  const want = enabled ? "on" : "off"
  const before = await waitFor(
    async () =>
      (await run.page.locator(css.linked).getAttribute("data-linked")) ===
      (enabled ? "off" : "on"),
    30_000,
  )
  if (!before) {
    failures.push(`linking was not ${enabled ? "off" : "on"} before the switch`)
    return false
  }
  let held = false
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
      (error) => (error?.refusal === "pairing_not_configured" ? "off" : String(error)),
    )
    if (answered !== want)
      failures.push(`the gateway answers linking ${answered}, not ${want}`)
    const saved = JSON.parse(readFileSync(config, "utf8")).native
    if (
      (saved?.enabled !== false) !== enabled ||
      saved?.listenAddress !== run.nativeListen
    )
      failures.push(`config.json holds native ${JSON.stringify(saved)}`)
    if (gateway.server.pid !== pid || gateway.server.exitCode !== null)
      failures.push("the gateway did not start again in the same process")
    held = failures.length === before
    return held
  })
  return held
}

/**
 * The places the gateway offers a new conversation (`agents.list`
 * `environments`), once `holds` is true of them; what they last were after
 * 30 s otherwise. A worker that connects or leaves changes them.
 */
const offers = (run, holds) =>
  waitFor(async () => {
    const { environments } = await run.client.agents.list()
    return holds(environments) ? environments : null
  }, 30_000).then(
    (environments) =>
      environments ?? run.client.agents.list().then((each) => each.environments),
  )

/**
 * Which worker of the pool holds `conversationId` now, as the gateway keeps
 * it: `bindings/<id>.json` beside its conversations, `{ worker, context }`.
 * `null` when it has none.
 */
function bindingOf(run, conversationId) {
  const file = join(
    run.stack.gateway.directory,
    "ci",
    "instances",
    INSTANCE,
    "conversations",
    "bindings",
    `${conversationId}.json`,
  )
  try {
    return JSON.parse(readFileSync(file, "utf8"))
  } catch (error) {
    if (error.code === "ENOENT") return null
    throw error
  }
}

/** What the window's heading names `worker` by, in the client's words. */
async function placeOf(worker) {
  const { environmentPlace } = await import("@nessa/client")
  return environmentPlace(worker.name)
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

/** What the window has told the host about waiting reviews (`fake-host.mjs`). */
const hostNotices = (page) => page.evaluate(() => window.__fakeHostNotices ?? null)

const sameNotice = (a, b) =>
  a.conversationId === b.conversationId &&
  a.executionId === b.executionId &&
  a.permissionId === b.permissionId

/**
 * The notice the host shows for the review now waiting in `conversationId`,
 * and that review read in full; `null` (a failure noted) when none is up
 * within 30 s. `not` is a notice of the same conversation already settled.
 */
async function noticePosted(run, conversationId, failures, not = null) {
  let found = null
  await waitFor(async () => {
    const view = await readWithReviews(run.client, conversationId).catch(() => null)
    const review = view?.permissions[0]
    const notices = await hostNotices(run.page)
    const notice =
      review &&
      notices?.up.find(
        (each) =>
          each.conversationId === conversationId &&
          each.executionId === review.executionId &&
          each.permissionId === review.permissionId,
      )
    if (notice && !(not && sameNotice(notice, not))) found = { notice, review }
    return Boolean(found)
  }, 30_000)
  if (!found) failures.push(`no notice was posted for the review in ${conversationId}`)
  return found
}

/** Waits for the host to take `notice` back once its review was `how`. */
async function noticeTakenBack(run, notice, how, failures) {
  const gone = await waitFor(async () => {
    const notices = await hostNotices(run.page)
    return (
      notices !== null &&
      !notices.up.some((each) => sameNotice(each, notice)) &&
      notices.withdrawn.some((each) => sameNotice(each, notice))
    )
  }, 15_000)
  if (!gone) failures.push(`the notice of a review ${how} was not taken back`)
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
  // Every review settled, so the host shows none of their notices.
  const cleared = await waitFor(async () => {
    const notices = await hostNotices(run.page)
    return notices !== null && notices.syncs > 0 && notices.up.length === 0
  }, 15_000)
  if (!cleared) {
    const notices = await hostNotices(run.page)
    failures.push(
      !notices?.syncs
        ? "the window never told the host which reviews wait"
        : `${notices.up.length} review notices are still up`,
    )
  }
  const text = await pageText(run.page)
  if (/display limit|Denied automatically/i.test(text))
    failures.push("the window says a review was hidden or denied automatically")
  return failures
}

async function startStack(options) {
  const knobs = knobsFrom(options)
  // Linking listens on every interface, as Settings sets it up, so the
  // gateway announces itself; clients on this machine dial it on loopback.
  const nativePort = await freePort()
  const nativeAddress = `127.0.0.1:${nativePort}`
  const nativeListen = `0.0.0.0:${nativePort}`
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
  // The `github` step's GitHub, on this machine, which the gateway reads its
  // app from, and Nessa's own shell tool, where an agent's git and gh run.
  let servers = []
  try {
    if (knobs.githubs > 0) servers = [shellServer(directory)]
  } catch (error) {
    rmSync(directory, { recursive: true, force: true })
    throw error
  }
  const github = knobs.githubs > 0 ? await startFakeGithub() : null
  const environment = github
    ? { NESSA_GITHUB_APP: github.app(join(directory, "github-app.json")) }
    : {}
  let stack
  try {
    stack = await startGatewayStack(options, INSTANCE, {
      environment,
      servers,
      webhooks:
        knobs.webhooks > 0
          ? [
              {
                principalId: "stress-webhook",
                url: "https://localhost/synthetic",
                macKey: Buffer.alloc(32, 7).toString("base64"),
              },
            ]
          : undefined,
      as: "panel",
      scenario,
      evidence: options.evidence,
      limits: { agentIdleSeconds: knobs.parks > 0 ? PARK_IDLE_SECONDS : 0 },
      // The agent a `handoff` step switches its conversation to.
      others: knobs.handoffs > 0 ? [otherAgent(options.agent)] : [],
      // Off, at a port of its own: the `linking` step turns it on from Settings.
      native: { enabled: false, listenAddress: nativeListen },
    })
  } catch (error) {
    await github?.close()
    rmSync(directory, { recursive: true, force: true })
    throw error
  }
  const close = stack.close
  stack.close = async () => {
    try {
      await close()
    } finally {
      await github?.close()
      rmSync(directory, { recursive: true, force: true })
    }
  }
  return panelTarget(stack, () => ({
    knobs,
    agent: options.agent,
    nativeAddress,
    nativeListen,
    github,
    scenario,
    // Removed when the stack closes.
    scratch: directory,
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
          [
            gatewayHost,
            {
              endpoint: stack.endpoint,
              credential: stack.credential,
              notices: true,
              starting: STARTING,
            },
          ],
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
