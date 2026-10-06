/**
 * A scenario turn, one test per row of the state table in
 * `scripted-scenario.mjs`, and the process that runs one: the gateway's
 * replies are written to the agent's stdin, as a gateway would.
 */
import { strict as assert } from "node:assert"
import { spawn } from "node:child_process"
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { createInterface } from "node:readline"
import { after, test } from "node:test"
import { fileURLToPath } from "node:url"

import { exited } from "./local-gateway.mjs"
import { CLAUDE_CALL_ID } from "./scripted-frames.mjs"
import {
  ALLOW_ONCE,
  DENY_ONCE,
  branchFor,
  parseScenario,
  permissionCall,
  promptText,
  scenarioScript,
  turnFor,
} from "./scripted-scenario.mjs"
import {
  TEXT_REPLY_APP_PROMPT,
  TEXT_REPLY_SCENARIO,
  WINDOW_SCENARIO,
} from "./scenarios.mjs"

const here = dirname(fileURLToPath(import.meta.url))
const mcptest = {
  name: "mcptest",
  command: process.execPath,
  args: [join(here, "server.mjs")],
  env: [],
}

const started = []
after(() => {
  for (const child of started) if (child.exitCode === null) child.kill("SIGKILL")
})

const windowScenario = () =>
  parseScenario(JSON.parse(readFileSync(WINDOW_SCENARIO, "utf8")))

/** The agent as `agent` running `scenario` (a file path or an object written to a temp file). */
function start(agent, scenario, env = {}) {
  const file =
    typeof scenario === "string"
      ? scenario
      : join(mkdtempSync(join(tmpdir(), "scripted-scenario-")), "scenario.json")
  if (typeof scenario !== "string") writeFileSync(file, JSON.stringify(scenario))
  const child = spawn(
    process.execPath,
    [join(here, "scripted-agent.mjs"), agent, "--scenario", file],
    { env: { ...process.env, ...env }, stdio: ["pipe", "pipe", "pipe"] },
  )
  started.push(child)
  const notes = []
  const incoming = []
  const waiting = []
  const requests = []
  const requestWaiters = []
  createInterface({ input: child.stdout }).on("line", (line) => {
    const message = JSON.parse(line)
    if (message.method && message.id !== undefined) {
      requests.push(message)
      requestWaiters.shift()?.(message)
      return
    }
    if (message.id === undefined) return notes.push(message)
    const wait = waiting.find((each) => each.id === message.id)
    if (!wait) return
    waiting.splice(waiting.indexOf(wait), 1)
    wait.done({ ...message, notes: notes.splice(0) })
  })
  let next = 1
  return {
    child,
    stderr: "",
    request: (method, params) =>
      new Promise((done, fail) => {
        const id = next++
        const timer = setTimeout(() => fail(new Error(`no answer to ${method}`)), 20_000)
        waiting.push({
          id,
          done: (answer) => {
            clearTimeout(timer)
            done(answer)
          },
        })
        child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`)
      }),
    notify: (method, params) =>
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method, params })}\n`),
    respond: (id, result) =>
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, result })}\n`),
    /** The next request the agent sends, or one already queued. */
    nextRequest: () =>
      new Promise((done, fail) => {
        const queued = requests.shift()
        if (queued) return done(queued)
        const timer = setTimeout(
          () => fail(new Error("the agent sent no request")),
          20_000,
        )
        requestWaiters.push((message) => {
          clearTimeout(timer)
          done(message)
        })
      }),
  }
}

const claudeOpen = {
  cwd: here,
  mcpServers: [mcptest],
  _meta: { claudeCode: { options: { model: "claude-test" } } },
}

test("parse rejects a scenario that is not a non-empty turn list", () => {
  for (const value of [
    {},
    { turns: [] },
    { turns: [{ steps: [] }] },
    { extra: true, turns: [{ steps: [{ do: "end" }] }] },
  ])
    assert.throws(() => parseScenario(value))
})

test("parse rejects a step the table does not have, and a branch that waits again", () => {
  assert.throws(() => parseScenario({ turns: [{ steps: [{ do: "sing" }] }] }))
  assert.throws(() =>
    parseScenario({
      turns: [
        {
          steps: [
            {
              do: "permission",
              tool: "report_rows",
              arguments: {},
              title: "Report",
              on: {
                "allow-once": [
                  {
                    do: "permission",
                    tool: "report_rows",
                    arguments: {},
                    title: "Again",
                    on: {},
                  },
                ],
              },
            },
          ],
        },
      ],
    }),
  )
  assert.throws(() =>
    parseScenario({
      turns: [
        {
          steps: [
            { do: "fail", message: "first" },
            { do: "text", chunks: ["after"] },
          ],
        },
      ],
    }),
  )
})

test("parse rejects a second turn with no when, which turnFor would never reach", () => {
  assert.throws(
    () =>
      parseScenario({
        turns: [
          { steps: [{ do: "end" }] },
          { steps: [{ do: "text", chunks: ["later"] }] },
        ],
      }),
    /two turns match any prompt/,
  )
})

test("parse rejects a later when that contains an earlier one", () => {
  assert.throws(
    () =>
      parseScenario({
        turns: [
          { when: "turn", steps: [{ do: "end" }] },
          { when: "fail the turn", steps: [{ do: "end" }] },
        ],
      }),
    /a turn matching fail the turn already matches turn/,
  )
  const specificFirst = parseScenario({
    turns: [
      { when: "fail the turn", steps: [{ do: "end" }] },
      { when: "turn", steps: [{ do: "text", chunks: ["General."] }] },
    ],
  })
  assert.equal(turnFor(specificFirst, "fail the turn").when, "fail the turn")
  assert.equal(turnFor(specificFirst, "the turn").when, "turn")
})

test("parse rejects two turns that match the same prompt, and an unknown answer", () => {
  assert.throws(() =>
    parseScenario({
      turns: [
        { when: "same", steps: [{ do: "end" }] },
        { when: "same", steps: [{ do: "end" }] },
      ],
    }),
  )
  assert.throws(() =>
    parseScenario({
      turns: [
        {
          steps: [
            {
              do: "permission",
              tool: "report_rows",
              arguments: {},
              title: "Report",
              on: { always: [{ do: "end" }] },
            },
          ],
        },
      ],
    }),
  )
})

test("the window scenario parses, and the first named turn wins over the fallback", () => {
  const scenario = windowScenario()
  assert.equal(turnFor(scenario, "please permission now").when, "permission")
  assert.equal(turnFor(scenario, "fail the turn now").when, "fail the turn")
  assert.equal(turnFor(scenario, "nothing"), null)
  const withFallback = parseScenario({
    turns: [
      { when: "named", steps: [{ do: "end" }] },
      { steps: [{ do: "text", chunks: ["fallback"] }] },
    ],
  })
  assert.equal(turnFor(withFallback, "a named prompt").when, "named")
  assert.equal(turnFor(withFallback, "other").when, undefined)
  assert.equal(
    promptText([
      { type: "text", text: "A" },
      { type: "image" },
      { type: "text", text: "B" },
    ]),
    "AB",
  )
})

test("scenarioScript reads the window file's steps, which are the check's expected strings", () => {
  const scenario = windowScenario()
  const script = scenarioScript(scenario)
  const permission = scenario.turns.find((turn) =>
    turn.steps.some((step) => step.do === "permission"),
  )
  const ask = permission.steps.find((step) => step.do === "permission")
  assert.equal(
    script.asking,
    permission.steps.find((step) => step.do === "text").chunks.join(""),
  )
  assert.equal(script.title, ask.title)
  assert.equal(
    script.allowed,
    ask.on[ALLOW_ONCE].find((step) => step.do === "text").chunks.join(""),
  )
  assert.equal(script.permission, permission.when)
  assert.deepEqual(
    scenario.turns.map((turn) => turn.when),
    ["permission", "fail the turn", "cancel the turn"],
  )
  assert.equal(script.beforeFailure, "Before the failure.")
  assert.equal(script.beforeCancel, "Before cancel.")
})

test("branchFor follows the table: selection, a later cancel, and a withdrawn review", () => {
  const on = { [ALLOW_ONCE]: [{ do: "end" }], [DENY_ONCE]: [{ do: "end" }] }
  assert.equal(
    branchFor({ outcome: { outcome: "selected", optionId: ALLOW_ONCE } }, false, on).kind,
    "branch",
  )
  assert.deepEqual(
    branchFor({ outcome: { outcome: "selected", optionId: ALLOW_ONCE } }, true, on),
    { kind: "stop", stopReason: "cancelled" },
  )
  assert.deepEqual(branchFor({ outcome: { outcome: "cancelled" } }, false, on), {
    kind: "stop",
    stopReason: "cancelled",
  })
  const withdrawn = branchFor({ outcome: { outcome: "cancelled" } }, false, {
    ...on,
    cancelled: [{ do: "text", chunks: ["Withdrawn."] }],
  })
  assert.equal(withdrawn.kind, "branch")
  assert.equal(
    branchFor({ outcome: { outcome: "selected", optionId: "other" } }, false, on).kind,
    "fail",
  )
  assert.equal(branchFor({}, false, on).kind, "fail")
})

test("permission frames name the tool the way each harness does", () => {
  const claude = permissionCall("claude", {
    id: "toolu_1",
    tool: "report_rows",
    title: "Report the rows",
    arguments: {},
  })
  assert.equal(claude.update._meta.claudeCode.toolName, "mcp__mcptest__report_rows")
  assert.deepEqual(claude.update.rawInput, {})
  const codex = permissionCall("codex", {
    id: "exec-1",
    tool: "report_rows",
    title: "Report the rows",
    arguments: { id: 1 },
  })
  assert.equal(codex.update._meta.is_mcp_tool_call, true)
  assert.deepEqual(codex.update.rawInput, {
    server: "mcptest",
    tool: "report_rows",
    arguments: { id: 1 },
  })
  assert.deepEqual(
    codex.request("session").options.map((option) => option.optionId),
    [ALLOW_ONCE, DENY_ONCE],
  )
})

test("text chunks share one message id, and a prompt the scenario does not answer fails", async () => {
  const agent = start("claude", {
    turns: [
      { when: "hello", steps: [{ do: "text", chunks: ["Hel", "lo"] }, { do: "end" }] },
    ],
  })
  await agent.request("initialize", {})
  const opened = await agent.request("session/new", claudeOpen)
  const sessionId = opened.result.sessionId
  const turn = await agent.request("session/prompt", {
    sessionId,
    prompt: [{ type: "text", text: "say hello" }],
  })
  assert.equal(turn.result.stopReason, "end_turn")
  const chunks = turn.notes.map((note) => note.params.update)
  assert.deepEqual(
    chunks.map((update) => update.content.text),
    ["Hel", "lo"],
  )
  assert.equal(chunks[0].messageId, chunks[1].messageId)
  const missed = await agent.request("session/prompt", {
    sessionId,
    prompt: [{ type: "text", text: "something else" }],
  })
  assert.match(missed.error.message, /does not answer/)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("allow runs the branch's tool call; deny does not; a cancel beats a later allow", async () => {
  const scenario = {
    turns: [
      {
        steps: [
          {
            do: "permission",
            tool: "report_rows",
            arguments: {},
            title: "Report the rows",
            on: {
              "allow-once": [
                { do: "tool", tool: "report_rows", arguments: {} },
                { do: "text", chunks: ["Allowed."] },
              ],
              "deny-once": [{ do: "text", chunks: ["Denied."] }],
            },
          },
          { do: "end" },
        ],
      },
    ],
  }
  const allow = start("claude", scenario)
  await allow.request("initialize", {})
  const allowSession = (await allow.request("session/new", claudeOpen)).result.sessionId
  const allowing = allow.request("session/prompt", {
    sessionId: allowSession,
    prompt: [],
  })
  const asked = await allow.nextRequest()
  assert.equal(asked.method, "session/request_permission")
  assert.equal(
    asked.params.toolCall._meta.claudeCode.toolName,
    "mcp__mcptest__report_rows",
  )
  allow.respond(asked.id, { outcome: { outcome: "selected", optionId: ALLOW_ONCE } })
  const allowed = await allowing
  assert.equal(allowed.result.stopReason, "end_turn")
  const updates = allowed.notes.map((note) => note.params.update)
  assert.equal(updates.at(-1).content.text, "Allowed.")
  const completion = updates.find((update) => update.status === "completed")
  assert.equal(completion.toolCallId, asked.params.toolCall.toolCallId)
  assert.equal(
    completion.content[0].content.text.includes("rows") ||
      completion.content[0].content.text.length > 0,
    true,
  )

  const deny = start("claude", scenario)
  await deny.request("initialize", {})
  const denySession = (await deny.request("session/new", claudeOpen)).result.sessionId
  const denying = deny.request("session/prompt", { sessionId: denySession, prompt: [] })
  const denial = await deny.nextRequest()
  deny.respond(denial.id, { outcome: { outcome: "selected", optionId: DENY_ONCE } })
  const denied = await denying
  assert.equal(denied.result.stopReason, "end_turn")
  assert.equal(
    denied.notes.some((note) => note.params.update.content?.text === "Denied."),
    true,
  )
  assert.equal(
    denied.notes.some(
      (note) =>
        note.params.update.toolCallId === denial.params.toolCall.toolCallId &&
        note.params.update.status === "failed",
    ),
    true,
  )

  const race = start("claude", scenario)
  await race.request("initialize", {})
  const raceSession = (await race.request("session/new", claudeOpen)).result.sessionId
  const racing = race.request("session/prompt", { sessionId: raceSession, prompt: [] })
  const pending = await race.nextRequest()
  race.notify("session/cancel", { sessionId: raceSession })
  race.respond(pending.id, { outcome: { outcome: "selected", optionId: ALLOW_ONCE } })
  const cancelled = await racing
  assert.equal(cancelled.result.stopReason, "cancelled")
  assert.equal(
    cancelled.notes.some((note) => note.params.update.content?.text === "Allowed."),
    false,
  )
})

test("end inside a branch ends the turn, and a branch that finishes runs the next step", async () => {
  const permission = (branch) => ({
    turns: [
      {
        steps: [
          {
            do: "permission",
            tool: "report_rows",
            arguments: {},
            title: "Report",
            on: { "allow-once": branch },
          },
          { do: "text", chunks: ["After."] },
        ],
      },
    ],
  })
  const ended = start("claude", permission([{ do: "end" }]))
  await ended.request("initialize", {})
  const endedId = (await ended.request("session/new", claudeOpen)).result.sessionId
  const ending = ended.request("session/prompt", { sessionId: endedId, prompt: [] })
  const endedAsk = await ended.nextRequest()
  ended.respond(endedAsk.id, { outcome: { outcome: "selected", optionId: ALLOW_ONCE } })
  const stopped = await ending
  assert.equal(stopped.result.stopReason, "end_turn")
  assert.equal(
    stopped.notes.some((note) => note.params.update.content?.text === "After."),
    false,
  )

  const continued = start("claude", permission([{ do: "text", chunks: ["Branch."] }]))
  await continued.request("initialize", {})
  const continuedId = (await continued.request("session/new", claudeOpen)).result
    .sessionId
  const continuing = continued.request("session/prompt", {
    sessionId: continuedId,
    prompt: [],
  })
  const continuedAsk = await continued.nextRequest()
  continued.respond(continuedAsk.id, {
    outcome: { outcome: "selected", optionId: ALLOW_ONCE },
  })
  const finished = await continuing
  assert.equal(finished.result.stopReason, "end_turn")
  const texts = finished.notes.map((note) => note.params.update.content?.text)
  assert.equal(texts.includes("Branch."), true)
  assert.equal(texts.includes("After."), true)
})

test("an unusable permission answer fails the announced call", async () => {
  const agent = start("claude", {
    turns: [
      {
        steps: [
          {
            do: "permission",
            tool: "report_rows",
            arguments: {},
            title: "Report",
            on: { "allow-once": [{ do: "end" }] },
          },
        ],
      },
    ],
  })
  await agent.request("initialize", {})
  const sessionId = (await agent.request("session/new", claudeOpen)).result.sessionId
  const turn = agent.request("session/prompt", { sessionId, prompt: [] })
  const asked = await agent.nextRequest()
  agent.respond(asked.id, { outcome: { outcome: "nope" } })
  const done = await turn
  assert.match(done.error.message, /not a selection or a cancellation/)
  assert.equal(
    done.notes.some(
      (note) =>
        note.params.update.toolCallId === asked.params.toolCall.toolCallId &&
        note.params.update.status === "failed",
    ),
    true,
  )
})

test("a tool step after a denial is a new call, and the denied call is failed", async () => {
  const agent = start("claude", {
    turns: [
      {
        steps: [
          {
            do: "permission",
            tool: "report_rows",
            arguments: {},
            title: "Report",
            on: { "deny-once": [{ do: "text", chunks: ["Denied."] }] },
          },
          { do: "tool", tool: "report_rows", arguments: {} },
          { do: "end" },
        ],
      },
    ],
  })
  await agent.request("initialize", {})
  const sessionId = (await agent.request("session/new", claudeOpen)).result.sessionId
  const turn = agent.request("session/prompt", { sessionId, prompt: [] })
  const asked = await agent.nextRequest()
  agent.respond(asked.id, { outcome: { outcome: "selected", optionId: DENY_ONCE } })
  const done = await turn
  assert.equal(done.result.stopReason, "end_turn")
  const updates = done.notes.map((note) => note.params.update)
  const deniedId = asked.params.toolCall.toolCallId
  assert.equal(
    updates.some(
      (update) => update.toolCallId === deniedId && update.status === "failed",
    ),
    true,
  )
  const completed = updates.find((update) => update.status === "completed")
  assert.notEqual(completed.toolCallId, deniedId)
})

test("a withdrawn review with no branch ends cancelled, and one with a branch runs it", async () => {
  const bare = start("claude", {
    turns: [
      {
        steps: [
          {
            do: "permission",
            tool: "report_rows",
            arguments: {},
            title: "Report",
            on: { "allow-once": [{ do: "text", chunks: ["Allowed."] }] },
          },
        ],
      },
    ],
  })
  await bare.request("initialize", {})
  const bareId = (await bare.request("session/new", claudeOpen)).result.sessionId
  const bareTurn = bare.request("session/prompt", { sessionId: bareId, prompt: [] })
  const bareAsk = await bare.nextRequest()
  bare.respond(bareAsk.id, { outcome: { outcome: "cancelled" } })
  assert.equal((await bareTurn).result.stopReason, "cancelled")

  const branched = start("claude", {
    turns: [
      {
        steps: [
          {
            do: "permission",
            tool: "report_rows",
            arguments: {},
            title: "Report",
            on: {
              "allow-once": [{ do: "end" }],
              cancelled: [{ do: "text", chunks: ["Withdrawn."] }],
            },
          },
        ],
      },
    ],
  })
  await branched.request("initialize", {})
  const branchedId = (await branched.request("session/new", claudeOpen)).result.sessionId
  const branchedTurn = branched.request("session/prompt", {
    sessionId: branchedId,
    prompt: [],
  })
  const branchedAsk = await branched.nextRequest()
  branched.respond(branchedAsk.id, { outcome: { outcome: "cancelled" } })
  const withdrawn = await branchedTurn
  assert.equal(withdrawn.result.stopReason, "end_turn")
  assert.equal(
    withdrawn.notes.some((note) => note.params.update.content?.text === "Withdrawn."),
    true,
  )
  assert.equal(
    withdrawn.notes.some(
      (note) =>
        note.params.update.toolCallId === branchedAsk.params.toolCall.toolCallId &&
        note.params.update.status === "failed",
    ),
    true,
  )
})

test("a failure after text fails the turn and keeps the text; wait-cancel ends cancelled", async () => {
  const failing = start("claude", {
    turns: [
      {
        steps: [
          { do: "text", chunks: ["Before the failure."] },
          { do: "fail", message: "the scenario stopped the turn" },
        ],
      },
    ],
  })
  await failing.request("initialize", {})
  const failId = (await failing.request("session/new", claudeOpen)).result.sessionId
  const failed = await failing.request("session/prompt", {
    sessionId: failId,
    prompt: [],
  })
  assert.match(failed.error.message, /the scenario stopped the turn/)
  assert.equal(failed.notes.at(-1).params.update.content.text, "Before the failure.")

  const waiting = start("claude", {
    turns: [
      { steps: [{ do: "text", chunks: ["Before cancel."] }, { do: "wait-cancel" }] },
    ],
  })
  await waiting.request("initialize", {})
  const waitId = (await waiting.request("session/new", claudeOpen)).result.sessionId
  const pending = waiting.request("session/prompt", { sessionId: waitId, prompt: [] })
  await new Promise((resolve) => setTimeout(resolve, 50))
  waiting.notify("session/cancel", { sessionId: waitId })
  const cancelled = await pending
  assert.equal(cancelled.result.stopReason, "cancelled")
  assert.equal(cancelled.notes.at(-1).params.update.content.text, "Before cancel.")
})

test("a second prompt while one is waiting is refused, and the first still answers", async () => {
  const agent = start("claude", {
    turns: [
      {
        steps: [
          {
            do: "permission",
            tool: "report_rows",
            arguments: {},
            title: "Report",
            on: { "allow-once": [{ do: "text", chunks: ["Allowed."] }] },
          },
        ],
      },
    ],
  })
  await agent.request("initialize", {})
  const sessionId = (await agent.request("session/new", claudeOpen)).result.sessionId
  const first = agent.request("session/prompt", { sessionId, prompt: [] })
  const asked = await agent.nextRequest()
  const second = await agent.request("session/prompt", { sessionId, prompt: [] })
  assert.match(second.error.message, /already in a prompt/)
  agent.respond(asked.id, { outcome: { outcome: "selected", optionId: ALLOW_ONCE } })
  const answered = await first
  assert.equal(answered.result.stopReason, "end_turn")
  assert.equal(
    answered.notes.some((note) => note.params.update.content?.text === "Allowed."),
    true,
  )
  assert.equal(
    answered.notes.some(
      (note) =>
        note.params.update.toolCallId === asked.params.toolCall.toolCallId &&
        note.params.update.status === "failed",
    ),
    true,
  )
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("codex allow carries no claude call id on the MCP call", async () => {
  const agent = start(
    "codex",
    {
      turns: [
        {
          steps: [
            {
              do: "permission",
              tool: "report_rows",
              arguments: {},
              title: "Report the rows",
              on: {
                "allow-once": [{ do: "tool", tool: "report_rows", arguments: {} }],
              },
            },
          ],
        },
      ],
    },
    { CODEX_CONFIG: JSON.stringify({ model: "gpt-test" }) },
  )
  await agent.request("initialize", {})
  const sessionId = (
    await agent.request("session/new", { cwd: here, mcpServers: [mcptest] })
  ).result.sessionId
  const pending = agent.request("session/prompt", { sessionId, prompt: [] })
  const asked = await agent.nextRequest()
  assert.equal(asked.params.toolCall._meta.is_mcp_tool_call, true)
  assert.equal(asked.params.toolCall._meta[CLAUDE_CALL_ID], undefined)
  agent.respond(asked.id, { outcome: { outcome: "selected", optionId: ALLOW_ONCE } })
  const turn = await pending
  assert.equal(turn.result.stopReason, "end_turn")
  assert.equal(
    turn.notes.some(
      (note) => note.params.update.toolCallId === asked.params.toolCall.toolCallId,
    ),
    true,
  )
})

test("text-reply answers any prompt with Ready, and show the server's app calls review_rows", () => {
  const scenario = parseScenario(JSON.parse(readFileSync(TEXT_REPLY_SCENARIO, "utf8")))
  const plain = turnFor(scenario, "Wabc: reply with exactly that word")
  assert.equal(plain?.when, undefined)
  assert.equal(plain?.steps[0].chunks.join(""), "Ready.")
  const app = turnFor(scenario, TEXT_REPLY_APP_PROMPT)
  assert.equal(app?.when, TEXT_REPLY_APP_PROMPT)
  assert.deepEqual(app?.steps[0], { do: "tool", tool: "review_rows", arguments: {} })
  const marked = turnFor(scenario, `A123: ${TEXT_REPLY_APP_PROMPT}`)
  assert.equal(marked, app)
})
