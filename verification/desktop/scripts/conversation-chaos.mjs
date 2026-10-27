#!/usr/bin/env node
import catalog from "../../../crates/nessa-sdk/data/models.json" with { type: "json" }
/**
 * The conversation chaos check: seeded random actions from several connected
 * clients at once, against a real gateway with the scripted agent standing in
 * for the model, and what must hold however they land.
 *
 * The stress check (`conversation-stress.mjs`) walks one person through a
 * planned conversation in the window. This one does no planning: each wave
 * draws a handful of actions (`lib/chaos-plan.mjs`) and sends them all at
 * once, to a few conversations, from a few clients standing in for one
 * person's windows and devices. The actions are what a person and their
 * agents can do to a conversation:
 *
 *   send      a message asking for a reply, a review, several reviews, a
 *             pause, a turn that runs until stopped, a failure, a thread
 *             handed work, or a thread read with more reads than allowed
 *   steer     a steering message into whatever runs
 *   note      a note kept, or handed to the next turn
 *   answer    allow, deny or withdraw a waiting review
 *   mode      change the access mode, running or not
 *   stop      stop the running turn or a queued one
 *   remove    take a queued message out
 *   reorder   reverse the queue (steering stays first)
 *   close     close the conversation's agent
 *   archive   archive the conversation, or unarchive it
 *   rename    rename the conversation: a plain title, one to fold, or one
 *             too long to keep whole
 *   read      read the view with every review it lists read in full
 *   restart   crash (SIGKILL) or shut down (SIGTERM) the gateway mid-wave
 *
 * Meanwhile `--followers` clients follow every conversation's events, the
 * first one's view and the list, stopping and following again as they go
 * (`lib/stress-followers.mjs`).
 *
 * After every wave: the gateway answers its health check, and has not
 * panicked. At the end, once every waiting review is allowed and every turn
 * that waits to be stopped is stopped, it must settle, and then:
 *
 *   - every conversation, the threads its agents made included, is at rest:
 *     nothing running, nothing queued, no review or question waiting, no
 *     access change left half made;
 *   - every message the gateway took is in its conversation's events as a
 *     turn that ended, once, unless it was taken out of the queue; one it
 *     refused is not there at all;
 *   - only a turn that asked to fail ended failed;
 *   - every note the gateway took can be found again;
 *   - a conversation renamed is listed, and read, under one of the titles
 *     it was renamed to, as the client's title rule made it;
 *   - no review was hidden for its size. One denied automatically is
 *     counted, not failed: the chaos answers reviews only as its dice say,
 *     and a review no one answers in time is denied by design;
 *   - every follower applied every conversation's events exactly once, in
 *     order, and its view of the first conversation is the one a read gives;
 *   - no action went unanswered, and none failed in a way no client is told
 *     to expect.
 *
 * Hooks are not exercised: they are the agent harness's own (native hooks),
 * and the scripted agent runs none. Neither are an agent's questions: the
 * scripted agent asks none. Sharing needs a second person's credential and
 * is left to the share checks.
 *
 * A failed run prints its seed and its actions, and then cuts them down to
 * the shortest list found that still fails (`--shrink` runs at most), and
 * writes that list where `--replay` takes it back.
 *
 * Build the gateway first (`cargo build -p nessa-server`).
 *
 *   pnpm test:e2e:chaos                           # the pull request's runs
 *   pnpm test:e2e:chaos -- --runs 20 --waves 200 --width 12 --seed 7
 *   pnpm test:e2e:chaos -- --replay <evidence>/shrunk-seed-7.json
 */
import { randomUUID } from "node:crypto"
import {
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"
import { parseArgs } from "node:util"

import { readWithReviews } from "../../../scripts/conversation-smoke/reviews.mjs"
import { startLocalGateway } from "../../../scripts/mcp-test-server/local-gateway.mjs"
import { turnEnded } from "../../../scripts/mcp-test-server/evidence.mjs"
import {
  STRESS,
  stressScenario,
} from "../../../scripts/mcp-test-server/scenarios/stress.mjs"
import { KNOBS, actions, describe, knobsFrom, random, shrink } from "./lib/chaos-plan.mjs"
import { connectClient, scriptedLaunch, waitFor } from "./lib/gateway-stack.mjs"
import { freePort } from "./lib/server.mjs"
import { Follower } from "./lib/stress-followers.mjs"

const INSTANCE = "conversation-chaos"
const AGENT = "claude"
/** The longest one action may take to answer; past it the action is stuck. */
const ACTION_MS = 90_000
/** How long the end may take to settle once nothing new is sent. */
const SETTLE_MS = 240_000
/** Turns that only end when stopped. Their successors after a restart quote them. */
const STOPPED_ONLY = [STRESS.wait, STRESS.cut]
const PANIC = /panicked at|thread '.*' panicked/

const log = (line) => process.stderr.write(`${line}\n`)

/** An error's code and message, for a failure line. */
const explain = (error) =>
  [error?.code, error?.message, error?.cause?.message ?? error?.cause?.code]
    .filter(Boolean)
    .join(" · ")

/**
 * Error codes an action may meet without anything being wrong: what was
 * acted on moved before the action landed, or a restart took the connection.
 * Anything else, and any error with no code, is a failure.
 */
const EXPECTED = new Set([
  // A review, turn or queue entry that moved on first.
  "stale_permission",
  "stale_question",
  "not_found",
  "not_running",
  "not_pending",
  "queue_changed",
  "priority_conflict",
  "invalid_state",
  "conflict",
  "busy",
  "conversation_busy",
  "no_running_turn",
  "approval_mode_busy",
  "approval_mode_unsupported",
  // A change whose acknowledgement was lost; the view says what it came to,
  // and the end checks no change is left waiting.
  "approval_mode_uncertain",
  // An access change waits for no turn to be running.
  "turn_running",
  // A stop that met a message still being admitted.
  "submission_unresolved",
  // The control lane was full; the code says send it again.
  "temporarily_unavailable",
  // An action on a conversation with no agent live opens it (its queue and
  // owed work are in storage), and past `limits.liveConversations`, with
  // every live agent at work or followed, none can be parked to make room.
  // The code says try again; an answer opens nothing and is stale instead.
  "conversation_capacity",
  // A restart.
  "disconnected",
  "connection_closed",
  "session_closed",
  "unavailable",
  "timeout",
])

/**
 * Codes only one kind of action may meet. A review is answered on the agent
 * that asked it: one that stopped after a failed turn, or was closed, took
 * its reviews with it, and a message sent next opens the conversation again.
 * A stop that meets the gateway closing an agent whose turn failed, to open
 * it again, is told the agent closed; the person stops again once it is open.
 */
const EXPECTED_FOR = {
  // Native pre-effect refusal (including Busy), or an uncertain receipt that
  // the final authoritative read must reconcile. Neither means it applied.
  model: new Set(["model_change_not_applied", "model_change_uncertain"]),
  // An access preset can be unavailable for the selected model, or a change
  // can settle not-applied. Final reads must still show no pending configuration;
  // neither outcome counts as applied.
  mode: new Set(["approval_mode_not_applied", "approval_mode_unavailable"]),
  answer: new Set(["conversation_closed"]),
  stop: new Set(["conversation_closed"]),
}

/** Errors after which the gateway may or may not have taken what was sent. */
const UNCERTAIN = new Set([
  "no code",
  "1006",
  "disconnected",
  "connection_closed",
  "session_closed",
  "unavailable",
  "timeout",
])

class Run {
  constructor(gateway, knobs) {
    this.gateway = gateway
    this.knobs = knobs
    this.clients = []
    this.slots = []
    /** executionId → what was sent and what the gateway said to it. */
    this.sent = new Map()
    this.notes = []
    /** Each rename sent, with whether the gateway applied it. */
    this.renames = []
    this.failures = []
    this.codes = new Map()
    this.restarts = 0
    /** Set while the gateway restarts: errors then are the restart's. */
    this.restarting = false
    this.followers = []
    this.counts = new Map()
    this.eventKinds = new Map()
    /** An action's index → the message or rename it sent, if it sent one. */
    this.byAction = new Map()
    /** When each action ran and how it ended, to read beside the gateway log. */
    this.journal = []
  }

  fail(line) {
    this.failures.push(line)
  }

  count(name) {
    this.counts.set(name, (this.counts.get(name) ?? 0) + 1)
  }

  /** The client in use as `n`, a new one when the old one gave up reconnecting. */
  async client(n) {
    const held = this.clients[n]
    // A client reconnecting after a restart takes no request until it is back.
    await waitFor(() => held.client.connectionState?.status !== "reconnecting", 30_000)
    if (held.client.connectionState?.status === "closed") {
      held.client.close()
      held.client = await connectClient(this.gateway, INSTANCE, held.as)
    }
    return held.client
  }

  /**
   * What an action's error says: expected ones are counted, the rest are
   * failures. A restart in flight excuses any error.
   */
  met(action, error, restarted) {
    const code = error?.code ?? "no code"
    this.codes.set(code, (this.codes.get(code) ?? 0) + 1)
    if (restarted || EXPECTED.has(code) || EXPECTED_FOR[action.kind]?.has(code)) return
    // A read or a reorder that meets a close of its conversation is refused
    // as closed rather than held up by the close: the close declines the
    // queue a reorder would change, and the next read opens it again.
    if (
      (action.kind === "read" || action.kind === "reorder") &&
      code === "conversation_closed" &&
      this.closing?.has(action.slot)
    )
      return
    this.fail(`action ${action.index} (${action.kind}) failed: ${explain(error)}`)
  }
}

const textOf = (run, action) =>
  `${STRESS[action.ask]} c${action.slot} a${action.index} s${run.knobs.seed}`

const ended = (turn) => turnEnded(turn.status)

/** A seeded choice for one action, so the same list picks the same things. */
const choose = (run, action, list) =>
  list[Math.floor(random(run.knobs.seed * 7_919 + action.index)() * list.length)]

const apply = {
  async send(run, action, client) {
    const conversationId = run.slots[action.slot]
    const executionId = randomUUID()
    const text = textOf(run, action)
    const entry = { text, slot: action.slot, ask: action.ask, kind: action.kind }
    run.sent.set(executionId, entry)
    run.byAction.set(action.index, entry)
    try {
      await client.conversation.submit(
        client.conversation.prepare(conversationId, text, undefined, undefined, {
          mode: action.kind === "steer" ? "steering" : "queued",
          executionId,
        }),
      )
      entry.taken = true
    } catch (error) {
      entry.error = error?.code ?? "no code"
      throw error
    }
  },

  steer(run, action, client) {
    return apply.send(run, action, client)
  },

  async note(run, action, client) {
    const requestId = randomUUID()
    const note = { slot: action.slot, requestId, index: action.index }
    run.notes.push(note)
    await client.conversation.message(
      run.slots[action.slot],
      `chaos note a${action.index} s${run.knobs.seed}`,
      action.delivery,
      { requestId },
    )
    note.taken = true
  },

  async answer(run, action, client) {
    const { permissionTargets } = await import("@nessa/client")
    const view = await readWithReviews(client, run.slots[action.slot])
    if (!view.permissions.length) return
    const targets = permissionTargets(view)
    const index = Math.floor(
      random(run.knobs.seed * 7_919 + action.index)() * targets.length,
    )
    const permission = view.permissions[index]
    if (action.answer === "withdraw") {
      await client.conversation.cancel(targets[index], "withdrawn by the chaos check")
      return
    }
    const option = permission.options.find((each) => each.effect === action.answer)
    if (!option) return
    await client.conversation.answer(targets[index], option.id)
  },

  async model(run, action, client) {
    const id = run.slots[action.slot]
    const requestId = randomUUID()
    const view = await client.conversation.read(id)
    const current = catalog.models.find((model) => model.modelId === view.runtime?.model)
    const offered = catalog.models.filter(
      (model) =>
        model.provider === current?.provider && model.modelId !== current.modelId,
    )
    if (!offered.length)
      throw new Error("no same-provider catalog target for model action")
    const target = choose(run, action, offered)
    const receipt = await client.conversation.setModel(id, target.modelId, { requestId })
    if (receipt.requestId !== requestId || receipt.model !== target.modelId)
      throw new Error("model receipt did not match its original request and target")
  },

  async mode(run, action, client) {
    await client.conversation.setApprovalMode(run.slots[action.slot], action.to)
  },

  async stop(run, action, client) {
    const { executionTargets } = await import("@nessa/client")
    const view = await client.conversation.read(run.slots[action.slot])
    const ids =
      action.which === "running"
        ? view.messages.filter((each) => !ended(each)).map((each) => each.executionId)
        : view.pending.map((each) => each.executionId)
    if (!ids.length) return
    const id = choose(run, action, ids)
    const target = executionTargets(view).find((each) => each.executionId === id)
    await client.conversation.stop(target)
  },

  async remove(run, action, client) {
    const { executionTargets } = await import("@nessa/client")
    const view = await client.conversation.read(run.slots[action.slot])
    if (!view.pending.length) return
    const id = choose(
      run,
      action,
      view.pending.map((each) => each.executionId),
    )
    const target = executionTargets(view).find((each) => each.executionId === id)
    const result = await client.conversation.remove(target)
    const entry = run.sent.get(id)
    if (entry && result?.applied !== false) entry.removed = true
  },

  async reorder(run, action, client) {
    const view = await client.conversation.read(run.slots[action.slot])
    if (!view.queueComplete || view.pending.length < 2) return
    const steering = view.pending.filter((each) => each.mode === "steering")
    const queued = view.pending.filter((each) => each.mode !== "steering")
    const order = [...steering.reverse(), ...queued.reverse()].map(
      (each) => each.executionId,
    )
    await client.conversation.reorder(run.slots[action.slot], order)
  },

  async close(run, action, client) {
    await client.conversation.close(run.slots[action.slot])
  },

  async archive(run, action, client) {
    const id = run.slots[action.slot]
    if (action.undo) await client.conversation.unarchive(id)
    else await client.conversation.archive(id)
  },

  async usageContinue(_run, action, client) {
    await client.conversation.setUsageLimitAutoContinue(action.enabled === true)
  },

  async rename(run, action, client) {
    const { conversationTitle } = await import("@nessa/client")
    const typed = {
      plain: `chaos a${action.index}`,
      folded: ` chaos\t\ta${action.index}\u0007  s${run.knobs.seed}\n`,
      long: `chaos a${action.index} ${"long ".repeat(20)}`,
    }[action.title]
    const rename = {
      slot: action.slot,
      index: action.index,
      title: conversationTitle(typed),
    }
    run.renames.push(rename)
    run.byAction.set(action.index, rename)
    const result = await client.conversation.rename(run.slots[action.slot], rename.title)
    rename.applied = result?.applied !== false
  },

  async read(run, action, client) {
    await readWithReviews(client, run.slots[action.slot])
  },

  async restart(run, action) {
    run.restarting = true
    try {
      await run.gateway.restart(action.signal)
      run.restarts += 1
    } finally {
      run.restarting = false
    }
  },
}

/** Runs one action, bounded: one that never answers is stuck, which is a failure. */
async function act(run, action) {
  run.count(action.kind)
  const restartsBefore = run.restarts
  const journaled = { at: new Date().toISOString(), ...action }
  run.journal.push(journaled)
  let timer
  try {
    const client = action.kind === "restart" ? null : await run.client(action.client)
    await Promise.race([
      apply[action.kind](run, action, client),
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(Object.assign(new Error("no answer"), { stuck: true })),
          ACTION_MS,
        )
      }),
    ])
  } catch (error) {
    if (error?.stuck) {
      run.fail(
        `action ${action.index} (${action.kind}) did not answer within ${ACTION_MS / 1000} s`,
      )
      return
    }
    if (action.kind === "restart") {
      run.fail(`the restart failed: ${explain(error)}`)
      return
    }
    const restarted = run.restarting || run.restarts !== restartsBefore
    // Whether a message the client saw fail was taken is not known when the
    // connection went with it: it may run, once.
    const entry = run.byAction.get(action.index)
    if (entry && (restarted || UNCERTAIN.has(error?.code ?? "no code")))
      entry.uncertain = true
    journaled.code = error?.code ?? "no code"
    run.met(action, error, restarted)
  } finally {
    journaled.done = new Date().toISOString()
    journaled.conversationId = run.slots[action.slot]
    clearTimeout(timer)
  }
}

/** What must hold after every wave: the gateway is up and has not panicked. */
async function between(run, wave) {
  const healthy = await waitFor(async () => {
    try {
      return (await fetch(`${run.gateway.url}/health`)).ok
    } catch {
      return false
    }
  }, 30_000)
  if (!healthy)
    run.fail(`after wave ${wave} the gateway does not answer its health check`)
  if (PANIC.test(run.gateway.log())) run.fail(`by wave ${wave} the gateway panicked`)
}

/** Every conversation the clients can list, the run's own and its agents' threads. */
async function everyConversation(run) {
  const ids = new Set(run.slots)
  for (let n = 0; n < run.clients.length; n += 1) {
    try {
      const client = await run.client(n)
      for (const archived of [false, true]) {
        const { conversations } = await client.conversation.list({ archived })
        for (const each of conversations) ids.add(each.conversationId)
      }
    } catch (error) {
      run.fail(`listing conversations failed: ${explain(error)}`)
    }
  }
  return [...ids]
}

/**
 * Whether `view` was read from a whole transcript. One read takes a bounded
 * number of saved records, and a view read short of the end does not list
 * what is queued, so it cannot show a conversation at rest.
 */
const whole = (view) =>
  view.transcriptState === "complete" || view.transcriptState === "complete_empty"

/**
 * Lets everything end: allows every waiting review and stops every turn that
 * only ends when stopped, until every conversation is at rest. Resolves with
 * the conversations' last views.
 *
 * Only conversations still moving are read again, a few at a time, and one
 * is read again at once after its reviews are answered: a turn asks its
 * reviews one after another, and each waits only so long. Past the cap on
 * live agents every read opens an agent and lets an idle one go, so one
 * pass over every conversation can take longer than a review waits. Once
 * nothing moves, every conversation is read once more to confirm it.
 */
async function settle(run) {
  const { executionTargets, permissionTargets } = await import("@nessa/client")
  const client = await run.client(0)
  const deadline = Date.now() + SETTLE_MS
  const views = new Map()
  /** Whether `id` is at rest after its reviews are answered, its stops asked. */
  const settleOne = async (id) => {
    for (let round = 0; round < 64 && Date.now() < deadline; round += 1) {
      let view
      try {
        view = await readWithReviews(client, id)
      } catch (error) {
        log(`settle: read ${id}: ${explain(error)}`)
        return false
      }
      views.set(id, view)
      const targets = permissionTargets(view)
      for (const [index, permission] of view.permissions.entries()) {
        const allow = permission.options.find((each) => each.effect === "allow")
        await (
          allow
            ? client.conversation.answer(targets[index], allow.id)
            : client.conversation.cancel(targets[index], "settling the chaos check")
        ).catch((error) => log(`settle: answer ${id}: ${explain(error)}`))
      }
      const running = view.messages.filter((each) => !ended(each))
      for (const turn of running) {
        if (STOPPED_ONLY.some((phrase) => turn.userText.includes(phrase))) {
          const target = executionTargets(view).find(
            (each) => each.executionId === turn.executionId,
          )
          await client.conversation
            .stop(target)
            .catch((error) => log(`settle: stop ${id}: ${explain(error)}`))
        }
      }
      // A review answered lets its turn ask the next one, and a view read
      // short of the end goes on from where it stopped: read it again now.
      // Read later, other conversations' reads let its agent go past the cap
      // on live agents, and opening it again writes past where it stopped.
      if (view.permissions.length || !whole(view)) continue
      return !(
        running.length ||
        view.pending.length ||
        view.questions.length ||
        view.approvalModeChange ||
        view.modelChange
      )
    }
    return false
  }
  let moving = await everyConversation(run)
  let confirming = true
  while (Date.now() < deadline) {
    const still = []
    const queue = [...moving]
    await Promise.all(
      Array.from({ length: 8 }, async () => {
        for (let id = queue.shift(); id; id = queue.shift())
          if (!(await settleOne(id))) still.push(id)
      }),
    )
    if (!still.length && confirming) return { views, quiet: true }
    // Nothing moved this pass: read everything once more to confirm it.
    confirming = !still.length
    moving = still.length ? still : await everyConversation(run)
    if (still.length) await sleep(500)
  }
  return { views, quiet: false }
}

/** Every event of `conversationId` from the start, read until nothing comes for `quietMs`. */
async function allEvents(client, conversationId, quietMs = 1_500) {
  const events = []
  let last = Date.now()
  let end = null
  const subscription = await client.subscriptions.events(
    conversationId,
    {
      batch(batch) {
        events.push(...batch.events)
        last = Date.now()
      },
      ended(why) {
        end = why
      },
    },
    { replay: true },
  )
  const deadline = Date.now() + 60_000
  // Quiet counts from the first batch: a replay of a long history can
  // take longer than the quiet window to start, and nothing read then is
  // not the same as nothing there.
  while (
    !end &&
    (events.length === 0 ? Date.now() - last < 15_000 : Date.now() - last < quietMs) &&
    Date.now() < deadline
  )
    await sleep(100)
  await subscription.close().catch(() => {})
  if (end) throw new Error(`a replay of ${conversationId} ended ${end.reason}`)
  return events
}

/** What must hold once everything has settled. */
async function check(run, { views, quiet }) {
  if (!quiet) {
    for (const [id, view] of views) {
      const running = view.messages.filter((each) => !ended(each))
      if (
        !whole(view) ||
        running.length ||
        view.pending.length ||
        view.permissions.length ||
        view.approvalModeChange ||
        view.modelChange
      )
        run.fail(
          `${id} is not at rest after ${SETTLE_MS / 1000} s: ${JSON.stringify({
            transcript: view.transcriptState,
            running: running.map((each) => [each.status, each.userText.slice(0, 50)]),
            pending: view.pending.map((each) => [each.mode, each.text.slice(0, 50)]),
            reviews: view.permissions.length,
            questions: view.questions.length,
            modeChange: view.approvalModeChange,
            modelChange: view.modelChange,
          })}`,
        )
    }
  }
  for (const [id, view] of views) {
    for (const turn of view.messages) {
      const said = `${turn.error ?? ""} ${(turn.parts ?? []).map((part) => part.text ?? "").join(" ")}`
      if (/Denied automatically|No human answered/i.test(said))
        run.autoDenied = (run.autoDenied ?? 0) + 1
      if (/display limit/i.test(said)) run.fail(`${id}: a review was hidden`)
    }
  }
  const client = await run.client(0)
  const outcomes = new Map()
  for (const [slot, conversationId] of run.slots.entries()) {
    let events
    try {
      events = await allEvents(client, conversationId)
    } catch (error) {
      run.fail(error.message)
      continue
    }
    for (const event of events)
      run.eventKinds.set(event.kind, (run.eventKinds.get(event.kind) ?? 0) + 1)
    // A turn's end is its local settlement (`turn.ended`), or, for one the
    // gateway or a person cancelled before it settled, its scheduling
    // cancellation (`state.changed`, status `cancelled`): the event contract
    // keeps `turn.ended` for settlements.
    const settled = new Map()
    const cancelled = new Set()
    for (const event of events) {
      if (!event.executionId) continue
      if (event.kind === "turn.ended")
        settled.set(event.executionId, [
          ...(settled.get(event.executionId) ?? []),
          event.outcome,
        ])
      else if (event.kind === "state.changed" && event.status === "cancelled")
        cancelled.add(event.executionId)
    }
    const view = views.get(conversationId)
    for (const [executionId, entry] of run.sent) {
      if (entry.slot !== slot) continue
      const end = settled.get(executionId) ?? []
      const outcome =
        end[0] ?? (cancelled.has(executionId) ? "cancelled before settling" : "none")
      const key = `${entry.kind} ${entry.ask}: ${entry.taken ? (entry.removed ? "removed" : "taken") : entry.error ? "errored" : "unanswered"} → ${outcome}`
      outcomes.set(key, (outcomes.get(key) ?? 0) + 1)
      const turn = view?.messages.find((each) => each.executionId === executionId)
      const said = `${JSON.stringify(entry.text)} (${executionId}${turn?.error ? `, ${turn.error}` : ""})`
      if (end.length > 1) run.fail(`${said} settled ${end.length} times: ${end}`)
      if (entry.taken && !entry.removed && outcome === "none") {
        const seen = events
          .filter((event) => event.executionId === executionId)
          .map((event) => `${event.kind}${event.status ? `:${event.status}` : ""}`)
        run.fail(
          `${said} was taken and never ended (events ${seen.join(",") || "none"}; view ${turn?.status ?? "none"})`,
        )
      }
      if (!entry.taken && entry.error && outcome !== "none" && !entry.uncertain)
        run.fail(`${said} was refused (${entry.error}) and still ran`)
      // Judged on where the turn ended, not on the first settlement: a turn
      // a restart cut off just after it was dispatched settles failed
      // (`Closed`) and is then recorded cancelled, and resumed, so the view
      // and every follower end on cancelled.
      // A turn past the view's window is judged from its events as the view
      // judges one (`record_status`): a failed settlement followed by a
      // cancellation, a turn a restart cut off and then resumed, ended
      // cancelled.
      const failed = turn
        ? turn.status === "failed"
        : end.includes("failed") && !cancelled.has(executionId)
      // A thread tool answered "temporarily_unavailable", which tells the
      // model to try again (a restart, or a thread still opening), or
      // "conversation_capacity" (every live agent at work); the scripted
      // agent gives up on any tool error instead. Counted, not failed.
      // A turn the bounded view no longer lists has no error to read: its
      // thread tool is all its events say (failed, or still pending when the
      // scripted agent gave up on its error before reporting it), and it is
      // counted apart.
      const threadTool = entry.ask === "watch" || entry.ask === "delegate"
      const unread =
        threadTool &&
        !turn &&
        events.some(
          (event) =>
            event.executionId === executionId &&
            event.kind === "tool.updated" &&
            (event.status === "failed" || event.status === "pending"),
        )
      const toldToRetry =
        unread ||
        (threadTool &&
          /failed: \{"error":"(temporarily_unavailable|conversation_capacity)"\}/.test(
            turn?.error ?? "",
          ))
      if (failed && toldToRetry) run.toolRetries = (run.toolRetries ?? 0) + 1
      if (failed && unread) run.toolFailuresUnread = (run.toolFailuresUnread ?? 0) + 1
      if (failed && entry.ask !== "fail" && !toldToRetry)
        run.fail(
          `${said} ended failed (events ${events
            .filter((event) => event.executionId === executionId)
            .map((event) => `${event.kind}${event.status ? `:${event.status}` : ""}`)
            .join(",")})`,
        )
    }
  }
  run.outcomes = Object.fromEntries([...outcomes].sort())
  for (const note of run.notes.filter((each) => each.taken)) {
    try {
      const status = await client.conversation.messageStatus(
        run.slots[note.slot],
        note.requestId,
      )
      if (!status.found)
        run.fail(
          `a note the gateway took cannot be found: action ${note.index}, ${note.requestId}`,
        )
    } catch (error) {
      run.fail(`a note's status could not be read: ${explain(error)}`)
    }
  }
  for (const [slot, conversationId] of run.slots.entries()) {
    // Renames land in any order, so the title is any one that may have
    // taken: applied, or whose answer a restart or a lost connection took.
    const titles = run.renames
      .filter((each) => each.slot === slot && (each.applied || each.uncertain))
      .map((each) => each.title)
    if (!titles.length) continue
    const listed = []
    for (const archived of [false, true]) {
      try {
        const { conversations } = await client.conversation.list({ archived })
        const found = conversations.find((each) => each.conversationId === conversationId)
        if (found) listed.push(found.title)
      } catch (error) {
        run.fail(`listing conversations failed: ${explain(error)}`)
      }
    }
    const read = views.get(conversationId)?.title
    if (listed.length !== 1 || !titles.includes(listed[0]))
      run.fail(
        `${conversationId} was renamed to ${JSON.stringify(titles)} and is listed as ${JSON.stringify(listed)}`,
      )
    else if (read !== listed[0])
      run.fail(
        `${conversationId} is listed as ${JSON.stringify(listed[0])} and read as ${JSON.stringify(read)}`,
      )
  }
  await Promise.all(run.followers.map((each) => each.settle()))
  for (const each of run.followers) run.failures.push(...(await each.check(client)))
  if (PANIC.test(run.gateway.log())) run.fail("the gateway panicked")
}

/**
 * One run of `list` on a gateway of its own. Resolves with what went wrong
 * and what was seen; writes the gateway's log under `evidence` when given.
 */
async function runOnce(list, knobs, { evidence, label }) {
  const directory = mkdtempSync(join(tmpdir(), "nessa-chaos-"))
  const scenario = join(directory, "stress.json")
  writeFileSync(
    scenario,
    JSON.stringify(stressScenario({ reviews: knobs.reviews, pauseMs: 1_500 })),
  )
  const launch = scriptedLaunch(AGENT, { scenario })
  const started = Date.now()
  let gateway
  try {
    gateway = await startLocalGateway({
      agent: AGENT,
      port: await freePort(),
      instance: INSTANCE,
      agentArgv: launch.command.argv,
      model: launch.command.model,
      path: launch.command.path,
      mcpServer: launch.mcp,
      signedOut: launch.signedOut,
      limits: knobs.live ? { liveConversations: knobs.live } : undefined,
      // Chaos leaves some reviews unanswered and counts the denials, at the
      // desk's thirty seconds rather than the phone's five minutes.
      reviewWaitSeconds: 30,
    })
  } catch (error) {
    rmSync(directory, { recursive: true, force: true })
    throw new Error(`the gateway did not start: ${error.message.split("\n")[0]}`, {
      cause: error,
    })
  }
  const run = new Run(gateway, knobs)
  // The log and journal so far, written after every wave as well as at the
  // end: a run the machine kills partway leaves what it saw up to then.
  const keep = () => {
    if (!evidence) return
    writeFileSync(join(evidence, `${label}.gateway.log`), gateway.log())
    writeFileSync(
      join(evidence, `${label}.journal.jsonl`),
      run.journal.map((each) => JSON.stringify(each)).join("\n"),
    )
  }
  try {
    for (let n = 0; n < knobs.clients; n += 1) {
      // One person's windows: each its own connection with the same credential.
      const as = "panel"
      run.clients.push({ as, client: await connectClient(gateway, INSTANCE, as) })
    }
    // Every conversation is created from every client at once: one opens it,
    // the others are told it is open.
    run.slots = Array.from({ length: knobs.conversations }, () => randomUUID())
    const agent = AGENT
    // More at once than a connection's slots are refused
    // `temporarily_unavailable`, which says to send it again.
    const create = async (client, conversationId) => {
      for (let tries = 1; ; tries += 1) {
        try {
          return await client.conversation.create({ conversationId, agent })
        } catch (error) {
          if (error?.code !== "temporarily_unavailable" || tries >= 50) throw error
          await sleep(100)
        }
      }
    }
    await Promise.all(
      run.slots.flatMap((conversationId) =>
        run.clients.map(({ client }) =>
          create(client, conversationId).catch((error) =>
            run.fail(`creating ${conversationId} failed: ${explain(error)}`),
          ),
        ),
      ),
    )
    const { subscriptionLimits } = await import("@nessa/client")
    run.followers = Array.from(
      { length: knobs.followers },
      (_, n) =>
        new Follower({
          n,
          connect: () => connectClient(gateway, INSTANCE, "panel"),
          limits: subscriptionLimits,
          random: random(knobs.seed * 1_000 + n + 1),
          mainId: run.slots[0],
        }),
    )
    await Promise.all(run.followers.map((each) => each.start()))
    const waves = [...new Set(list.map((action) => action.wave))].sort((a, b) => a - b)
    for (const wave of waves) {
      const actions = list.filter((action) => action.wave === wave)
      run.closing = new Set(
        actions.filter((action) => action.kind === "close").map((action) => action.slot),
      )
      await Promise.all(actions.map((action) => act(run, action)))
      await between(run, wave)
      keep()
    }
    await check(run, await settle(run))
  } catch (error) {
    run.fail(`the run stopped: ${explain(error)}`)
  } finally {
    // A failed run keeps the gateway's saved state, to read what each turn
    // recorded (its final error, its scheduling) beside the log.
    if (evidence && run.failures.length) {
      try {
        cpSync(join(gateway.directory, "ci"), join(evidence, `${label}.state`), {
          recursive: true,
        })
      } catch (error) {
        log(`the gateway's state was not kept: ${explain(error)}`)
      }
    }
    await Promise.allSettled(run.followers.map((each) => each.close()))
    for (const { client } of run.clients) client.close()
    await gateway.stop().catch(() => {})
    keep()
    rmSync(directory, { recursive: true, force: true })
  }
  return {
    failures: run.failures,
    seen: {
      seconds: Math.round((Date.now() - started) / 1000),
      actions: Object.fromEntries([...run.counts].sort()),
      errors: Object.fromEntries([...run.codes].sort()),
      restarts: run.restarts,
      outcomes: run.outcomes,
      toolRetries: run.toolRetries ?? 0,
      toolFailuresUnread: run.toolFailuresUnread ?? 0,
      autoDenied: run.autoDenied ?? 0,
      events: Object.fromEntries(run.eventKinds),
      notes: run.notes.filter((each) => each.taken).length,
      followers: run.followers.map((each) => ({
        conversations: each.events.size,
        ...each.counts,
      })),
    },
  }
}

// `pnpm test:e2e:chaos -- --seed 7` passes the `--` on.
const args = process.argv.slice(2).filter((arg, i) => !(i === 0 && arg === "--"))
const options = parseArgs({
  args,
  options: {
    runs: { type: "string" },
    replay: { type: "string" },
    evidence: { type: "string" },
    help: { type: "boolean" },
    ...Object.fromEntries(Object.keys(KNOBS).map((name) => [name, { type: "string" }])),
  },
}).values

if (options.help) {
  process.stdout
    .write(`Usage: node verification/desktop/scripts/conversation-chaos.mjs [options]

Needs the gateway built (cargo build -p nessa-server; or MCP_LIVE_NESSA).

Numbers (a flag, else NESSA_CHAOS_<NAME>, else the default):
${Object.entries(KNOBS)
  .map(
    ([name, knob]) =>
      `  --${name.padEnd(13)} ${String(knob.default).padStart(3)}  ${knob.what}`,
  )
  .join("\n")}
  --runs          2  runs, each on a gateway of its own, seeds counting up from --seed
                     (or NESSA_CHAOS_RUNS)

  --replay <file>   run the actions a failed run wrote, with its numbers
  --evidence <dir>  write each run's gateway log, report and shrunk actions there
`)
  process.exit(0)
}

const evidence = options.evidence
if (evidence) mkdirSync(evidence, { recursive: true })

/**
 * What kind of failure a line is, without the names, numbers and texts that
 * change from run to run: shrinking keeps a cut only while it fails the same
 * way, not merely in some way.
 */
const signature = (line) =>
  line
    .replace(/"(?:[^"\\]|\\.)*"/g, '"…"')
    .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, "<id>")
    .replace(/\(events [^)]*\)/g, "(events …)")
    .replace(/\d+/g, "N")

/** Runs `list` once and says whether it failed as `wanted` did, for shrinking. */
const failsWith = (knobs, wanted) => async (list) => {
  const { failures } = await runOnce(list, knobs, { label: "shrinking" })
  const kinds = new Set(failures.map(signature))
  const same = [...wanted].some((kind) => kinds.has(kind))
  log(
    `  ${list.length} actions: ${same ? "fails the same way" : failures.length ? "fails another way" : "holds"}`,
  )
  return same
}

const report = []
let failed = false
if (options.replay) {
  const saved = JSON.parse(readFileSync(options.replay, "utf8"))
  const knobs = saved.knobs
  log(
    `replaying ${saved.actions.length} actions of seed ${knobs.seed}\n${describe(saved.actions)}`,
  )
  const result = await runOnce(saved.actions, knobs, {
    evidence,
    label: `replay-seed-${knobs.seed}`,
  })
  report.push({ seed: knobs.seed, replay: options.replay, ...result })
  failed = result.failures.length > 0
} else {
  const base = knobsFrom(options)
  const runs = Number(options.runs ?? process.env.NESSA_CHAOS_RUNS ?? 2)
  if (!Number.isSafeInteger(runs) || runs < 1) throw new Error("runs is 1 or more")
  for (let n = 0; n < runs; n += 1) {
    const knobs = { ...base, seed: base.seed + n }
    const list = actions(knobs)
    log(`chaos seed ${knobs.seed}: ${JSON.stringify(knobs)}`)
    const result = await runOnce(list, knobs, { evidence, label: `seed-${knobs.seed}` })
    const entry = { seed: knobs.seed, ...result }
    report.push(entry)
    log(
      `${result.failures.length ? "FAILED" : "held  "} seed ${knobs.seed} in ${result.seen.seconds} s: ${JSON.stringify(result.seen)}`,
    )
    if (!result.failures.length) continue
    failed = true
    log(
      `seed ${knobs.seed} failed:\n  ${result.failures.join("\n  ")}\nactions:\n${describe(list)}`,
    )
    if (knobs.shrink > 0) {
      log(
        `cutting seed ${knobs.seed}'s ${list.length} actions down, ${knobs.shrink} runs at most…`,
      )
      const wanted = new Set(result.failures.map(signature))
      const file = evidence && join(evidence, `shrunk-seed-${knobs.seed}.json`)
      const save = (actions) => {
        if (file) writeFileSync(file, `${JSON.stringify({ knobs, actions }, null, 2)}\n`)
      }
      save(list)
      const { list: small, tried } = await shrink(
        list,
        failsWith(knobs, wanted),
        knobs.shrink,
        save,
      )
      entry.shrunk = { actions: small.length, tried }
      log(
        `seed ${knobs.seed} still fails with ${small.length} actions (${tried} runs):\n${describe(small)}`,
      )
      if (file) {
        log(`replay it: pnpm test:e2e:chaos -- --replay ${file}`)
      } else {
        log(`replay it from a file holding: ${JSON.stringify({ knobs, actions: small })}`)
      }
    }
  }
}
const summary = JSON.stringify(report, null, 2)
if (evidence) writeFileSync(join(evidence, "chaos-report.json"), `${summary}\n`)
process.stdout.write(`${summary}\n`)
const held = report.filter((each) => each.failures.length === 0).length
log(`conversation-chaos: ${held}/${report.length} runs held`)
process.exit(failed ? 1 : 0)
