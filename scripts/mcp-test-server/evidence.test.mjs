import { strict as assert } from "node:assert"
import { test } from "node:test"
import { lines, record } from "./acp-recorder.mjs"
import {
  allowOnce,
  frameShape,
  permissionDecisions,
  parseRecording,
  toolFrames,
  turnEnded,
  uiMentions,
  viewTools,
  givenServers,
} from "./evidence.mjs"

const update = (sessionUpdate, extra = {}) => ({
  jsonrpc: "2.0",
  method: "session/update",
  params: { sessionId: "s", update: { sessionUpdate, toolCallId: "t", ...extra } },
})

test("the recorder logs each frame with its direction, and text that is not JSON as text", () => {
  assert.deepEqual(JSON.parse(record("to-agent", '{"id":1}')), {
    direction: "to-agent",
    frame: { id: 1 },
  })
  assert.deepEqual(JSON.parse(record("from-agent", "not json")), {
    direction: "from-agent",
    text: "not json",
  })
  const seen = []
  let rest = lines("", '{"a":1}\n{"b"', (line) => seen.push(line))
  rest = lines(rest, ":2}\n\n", (line) => seen.push(line))
  assert.deepEqual(seen, ['{"a":1}', '{"b":2}'])
  assert.equal(rest, "")
})

test("only the agent's tool updates are read as tool frames", () => {
  const records = parseRecording(
    [
      record("from-agent", JSON.stringify(update("tool_call", { title: "x" }))),
      record("from-agent", JSON.stringify(update("agent_message_chunk"))),
      record("to-agent", JSON.stringify(update("tool_call"))),
      "broken line\n",
      record(
        "from-agent",
        JSON.stringify(update("tool_call_update", { status: "failed" })),
      ),
    ].join(""),
  )
  assert.deepEqual(
    toolFrames(records).map((frame) => frame.sessionUpdate),
    ["tool_call", "tool_call_update"],
  )
  assert.deepEqual(
    frameShape({
      sessionUpdate: "tool_call_update",
      toolCallId: "t",
      content: [{ type: "content", content: { type: "text" } }, { type: "diff" }],
    }).content,
    ["content:text", "diff"],
  )
})

test("any ui:// resource or resourceUri the agent sends is found, and none is not invented", () => {
  const records = [
    {
      direction: "from-agent",
      frame: update("tool_call", { _meta: { ui: { resourceUri: "x" } } }),
    },
    { direction: "from-agent", frame: { result: { text: "see ui://app/a.html" } } },
    { direction: "to-agent", frame: { params: { uri: "ui://asked/for" } } },
  ]
  assert.deepEqual(uiMentions(records), [
    "[0].params.update._meta.ui.resourceUri",
    "[1].result.text",
  ])
  assert.deepEqual(uiMentions(records.slice(2)), [])
})

test("the view's MCP tools are those naming the server", () => {
  const view = {
    tools: [
      { title: "a", mcp: { server: "mcptest", tool: "a" } },
      { title: "b" },
      { title: "c", mcp: { server: "other", tool: "c" } },
    ],
  }
  assert.deepEqual(
    viewTools(view, "mcptest").map((tool) => tool.title),
    ["a"],
  )
  assert.deepEqual(viewTools(null, "mcptest"), [])
})

test("only a call to the test server's tools is allowed, with its allow-once option", () => {
  const options = [
    { id: "allow_always", label: "Always allow" },
    { id: "allow_once", label: "Allow" },
    { id: "reject_once", label: "Don't allow" },
  ]
  const view = {
    tools: [
      { executionId: "e", toolId: "mcp", mcp: { server: "mcptest", tool: "x" } },
      { executionId: "e", toolId: "shell" },
      { executionId: "e", toolId: "other", mcp: { server: "other", tool: "x" } },
    ],
  }
  const ask = (toolId, offered = options) => ({
    executionId: "e",
    toolId,
    toolName: "execute",
    options: offered,
  })
  assert.equal(allowOnce(view, ask("mcp"), "mcptest")?.id, "allow_once")
  assert.equal(
    allowOnce(view, ask("mcp", [{ id: "allow-once", label: "x" }]), "mcptest")?.id,
    "allow-once",
  )
  for (const toolId of ["shell", "other", "unknown"])
    assert.equal(allowOnce(view, ask(toolId), "mcptest"), null, toolId)
  // Never a standing approval, nor an option merely labelled as allowing.
  assert.equal(allowOnce(view, ask("mcp", options.slice(0, 1)), "mcptest"), null)
  assert.equal(
    allowOnce(view, ask("mcp", [{ id: "x", label: "Allow once" }]), "mcptest"),
    null,
  )
})

test("each permission is decided once: allowed if it is the server's, reported if not", () => {
  const view = {
    tools: [
      { executionId: "e", toolId: "mcp", mcp: { server: "mcptest", tool: "x" } },
      { executionId: "e", toolId: "shell" },
    ],
    permissions: [
      {
        executionId: "e",
        permissionId: "p1",
        toolId: "mcp",
        options: [{ id: "allow_once" }],
      },
      {
        executionId: "e",
        permissionId: "p2",
        toolId: "shell",
        options: [{ id: "allow_once" }],
      },
    ],
  }
  const answered = new Set()
  const first = permissionDecisions(view, answered, "mcptest")
  assert.deepEqual(
    first.allow.map(({ key, option }) => [key, option.id]),
    [["e:p1", "allow_once"]],
  )
  assert.deepEqual(
    first.declined.map(({ key }) => key),
    ["e:p2"],
  )
  answered.add("e:p1")
  const again = permissionDecisions(view, answered, "mcptest")
  assert.deepEqual(again.allow, [])
  assert.deepEqual(permissionDecisions(null, answered, "mcptest"), {
    allow: [],
    declined: [],
  })
})

test("the recorder passes everything through to a slow reader, splits nothing, and reports a signal", async () => {
  const { spawnSync } = await import("node:child_process")
  const { mkdtempSync, readFileSync, rmSync } = await import("node:fs")
  const { tmpdir } = await import("node:os")
  const { join } = await import("node:path")
  const { fileURLToPath } = await import("node:url")
  const directory = mkdtempSync(join(tmpdir(), "acp-recorder-"))
  try {
    const log = join(directory, "log.jsonl")
    const recorder = fileURLToPath(new URL("./acp-recorder.mjs", import.meta.url))
    // 4 MB out of the agent, read by a consumer that sleeps first. Forwarding
    // with unpiped writes and exiting on the agent's close lost all but the
    // first 64 KB here on macOS; this is the run that showed it.
    const writer =
      'for(let i=0;i<20000;i++)process.stdout.write(JSON.stringify({i,pad:"x".repeat(200)})+"\\n")'
    const run = spawnSync(
      "/bin/sh",
      [
        "-c",
        `"${process.execPath}" "${recorder}" "${log}" "${process.execPath}" -e '${writer}' </dev/null | (sleep 1; wc -c)`,
      ],
      { encoding: "utf8" },
    )
    const expected = Array.from(
      { length: 20000 },
      (_, i) => JSON.stringify({ i, pad: "x".repeat(200) }) + "\n",
    ).join("").length
    assert.equal(Number(run.stdout.trim()), expected)
    assert.equal(parseRecording(readFileSync(log, "utf8")).length, 20000)
    // A character split across two writes is passed through and logged whole.
    rmSync(log)
    const split = spawnSync(
      process.execPath,
      [
        recorder,
        log,
        process.execPath,
        "-e",
        "process.stdout.write(Buffer.from([0x22,0xc3]));setTimeout(()=>process.stdout.write(Buffer.from([0xa9,0x22,0x0a])),50)",
      ],
      { input: "" },
    )
    assert.deepEqual([...split.stdout], [0x22, 0xc3, 0xa9, 0x22, 0x0a])
    assert.deepEqual(parseRecording(readFileSync(log, "utf8"))[0].frame, "é")
    const signalled = spawnSync(
      process.execPath,
      [recorder, log, process.execPath, "-e", 'process.kill(process.pid,"SIGTERM")'],
      { input: "" },
    )
    assert.equal(signalled.status, 128 + 15)
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("the recorder logs a last line that has no newline, and exits with the agent's status", async () => {
  const { spawnSync } = await import("node:child_process")
  const { mkdtempSync, readFileSync } = await import("node:fs")
  const { tmpdir } = await import("node:os")
  const { join } = await import("node:path")
  const { fileURLToPath } = await import("node:url")
  const directory = mkdtempSync(join(tmpdir(), "acp-recorder-"))
  const log = join(directory, "log.jsonl")
  try {
    const recorder = fileURLToPath(new URL("./acp-recorder.mjs", import.meta.url))
    const echo = [process.execPath, "-e", "process.stdin.pipe(process.stdout)"]
    const run = spawnSync(process.execPath, [recorder, log, ...echo], {
      input: '{"a":1}\n{"b":"é"}',
      encoding: "utf8",
    })
    assert.equal(run.status, 0)
    assert.equal(run.stdout, '{"a":1}\n{"b":"é"}')
    assert.deepEqual(
      parseRecording(readFileSync(log, "utf8")).map((each) => [
        each.direction,
        each.frame,
      ]),
      [
        ["to-agent", { a: 1 }],
        ["to-agent", { b: "é" }],
        ["from-agent", { a: 1 }],
        ["from-agent", { b: "é" }],
      ],
    )
    const failing = spawnSync(
      process.execPath,
      [recorder, log, process.execPath, "-e", "process.exit(3)"],
      { input: "" },
    )
    assert.equal(failing.status, 3)
  } finally {
    ;(await import("node:fs")).rmSync(directory, { recursive: true, force: true })
  }
})

test("the recorder exits with the agent's status when the agent stops reading its input", async () => {
  const { spawnSync } = await import("node:child_process")
  const { mkdtempSync, readFileSync, rmSync } = await import("node:fs")
  const { tmpdir } = await import("node:os")
  const { join } = await import("node:path")
  const { fileURLToPath } = await import("node:url")
  const directory = mkdtempSync(join(tmpdir(), "acp-recorder-"))
  try {
    const log = join(directory, "log.jsonl")
    const recorder = fileURLToPath(new URL("./acp-recorder.mjs", import.meta.url))
    const run = spawnSync(
      process.execPath,
      [
        recorder,
        log,
        process.execPath,
        "-e",
        'process.stdout.write("{\\"bye\\":1}\\n");setTimeout(()=>process.exit(5),200)',
      ],
      { input: '{"x":1}\n'.repeat(200_000), encoding: "utf8" },
    )
    assert.equal(run.status, 5)
    assert.equal(run.stdout, '{"bye":1}\n')
    const logged = parseRecording(readFileSync(log, "utf8"))
    assert.ok(
      logged.some((each) => each.direction === "from-agent" && each.frame?.bye === 1),
    )
    // Only what could reach the agent is logged as sent to it: a pipe's worth,
    // not the 200,000 frames offered.
    assert.ok(logged.filter((each) => each.direction === "to-agent").length < 50_000)
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("a signal ends the recorder, and the agent then sees its input close", async () => {
  const { spawn } = await import("node:child_process")
  const { once } = await import("node:events")
  const { mkdtempSync, rmSync } = await import("node:fs")
  const { tmpdir } = await import("node:os")
  const { join } = await import("node:path")
  const { fileURLToPath } = await import("node:url")
  const directory = mkdtempSync(join(tmpdir(), "acp-recorder-"))
  const marker = join(directory, "agent-saw-eof")
  try {
    const recorder = fileURLToPath(new URL("./acp-recorder.mjs", import.meta.url))
    const agent =
      'process.stdout.write("ready\\n");process.stdin.resume();' +
      `process.stdin.on("end",()=>{require("fs").writeFileSync(${JSON.stringify(marker)},"");process.exit(0)})`
    const run = spawn(process.execPath, [
      recorder,
      join(directory, "log.jsonl"),
      process.execPath,
      "-e",
      agent,
    ])
    run.stdout.setEncoding("utf8")
    await once(run.stdout, "data")
    run.kill("SIGTERM")
    const [, signal] = await once(run, "exit")
    assert.equal(signal, "SIGTERM")
    const { existsSync } = await import("node:fs")
    for (let i = 0; i < 100 && !existsSync(marker); i += 1)
      await new Promise((done) => setTimeout(done, 20))
    assert.ok(existsSync(marker))
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("the servers given to the harness are read from each session it opened", () => {
  const relay = {
    name: "mcptest",
    command: "/nessa",
    args: ["mcp-relay", "/s", "mcptest", "sha256:a"],
  }
  const records = [
    {
      direction: "to-agent",
      frame: { method: "session/new", params: { mcpServers: [{ ...relay, env: [] }] } },
    },
    {
      direction: "to-agent",
      frame: { method: "session/load", params: { mcpServers: [relay] } },
    },
    // What the agent sends, and other methods, are not what it was given.
    {
      direction: "from-agent",
      frame: { method: "session/new", params: { mcpServers: [{ name: "x" }] } },
    },
    {
      direction: "to-agent",
      frame: { method: "session/prompt", params: { mcpServers: [{ name: "y" }] } },
    },
    { direction: "to-agent", frame: { method: "session/new", params: {} } },
  ]
  assert.deepEqual(givenServers(records), [relay, relay])
})

test("a turn has ended when completed, failed or cancelled; unresolved, running and queued have not (#449)", () => {
  for (const status of ["completed", "failed", "cancelled"])
    assert.equal(turnEnded(status), true)
  for (const status of ["unresolved", "running", "queued", "injected", undefined])
    assert.equal(turnEnded(status), false)
})
