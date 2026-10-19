import assert from "node:assert/strict"
import test from "node:test"
import { withFailureAnnotations } from "./diagnostics.mjs"

function message(line) {
  assert.equal(line.split("\n").length, 2, "one workflow command line")
  assert.match(line, /^::error title=Cargo test process failed::/)
  return decodeURIComponent(line.slice(line.indexOf("::", 2) + 2).trimEnd())
}

test("failure annotation retains split stdout/stderr, exact invocation and exit", async () => {
  const original = { cwd: "/synthetic", marker: Symbol("unchanged") }
  const forwarded = []
  const commands = []
  const args = ["--synthetic", "space argument"]
  const wrapped = withFailureAnnotations(
    async (command, receivedArgs, io) => {
      assert.equal(command, "fixture.exe")
      assert.equal(receivedArgs, args)
      assert.equal(io.cwd, original.cwd)
      assert.equal(io.marker, original.marker)
      io.onStdout("test synthetic::fails ... FAI")
      io.onStderr("LED\nthread 'synthetic::fails' panicked\n")
      io.onStderr("assertion `left == right` failed\n")
      return 101
    },
    (line) => commands.push(line),
  )
  const code = await wrapped("fixture.exe", args, {
    ...original,
    onStdout: (chunk) => forwarded.push(["out", chunk]),
    onStderr: (chunk) => forwarded.push(["err", chunk]),
  })
  assert.equal(code, 101)
  assert.equal(commands.length, 1)
  assert.match(
    message(commands[0]),
    /fixture\.exe \["--synthetic","space argument"\] exited with 101/,
  )
  assert.match(
    message(commands[0]),
    /FAILED\nthread 'synthetic::fails' panicked\nassertion `left == right` failed/,
  )
  assert.deepEqual(
    forwarded.map(([stream]) => stream),
    ["out", "err", "err"],
  )
  assert.equal(forwarded[0][1], "test synthetic::fails ... FAI")
})

test("output cannot inject workflow commands; percent and CR/LF survive decoding", async () => {
  const commands = []
  const text = "100%\r\n::error title=Injected::payload%0Aend"
  await withFailureAnnotations(
    async (_command, _args, io) => {
      io.onStdout(text)
      return 7
    },
    (line) => commands.push(line),
  )("fixture", [], {})
  assert.equal(commands.length, 1)
  assert.match(commands[0], /100%25%0D%0A::error/)
  assert.ok(message(commands[0]).endsWith(text))
})

test("bounded output names truncation, keeps the final assertion and forwards all output", async () => {
  let annotation = ""
  let forwarded = ""
  const output =
    "discarded-prefix" + "x".repeat(40_000) + "\nassertion failed: synthetic-tail"
  await withFailureAnnotations(
    async (_command, _args, io) => {
      io.onStdout(output.slice(0, 100))
      io.onStdout(output.slice(100))
      return 2
    },
    (line) => {
      annotation = line
    },
  )("fixture", [], {
    onStdout: (chunk) => {
      forwarded += chunk
    },
  })
  assert.equal(forwarded, output)
  const text = message(annotation)
  assert.match(text, /Output truncated; last 16384 characters:/)
  assert.doesNotMatch(text, /discarded-prefix/)
  assert.ok(text.endsWith(output.slice(-16_384)))
  assert.ok(text.length < 16_600)
})

test("a successful child is not reclassified by failure-shaped output", async () => {
  let annotations = 0
  const result = await withFailureAnnotations(
    async (_command, _args, io) => {
      io.onStderr("FAILED\nthread panicked\n::error::synthetic\n")
      return 0
    },
    () => {
      annotations += 1
    },
  )("fixture", [], {})
  assert.equal(result, 0)
  assert.equal(annotations, 0)
})

test("spawn rejection preserves the same error and annotates its output", async () => {
  const failure = new Error("synthetic spawn refusal")
  let annotation = ""
  const wrapped = withFailureAnnotations(
    async (_command, _args, io) => {
      io.onStderr("synthetic diagnostic")
      throw failure
    },
    (line) => {
      annotation = line
    },
  )
  await assert.rejects(wrapped("fixture", [], {}), (error) => error === failure)
  assert.match(message(annotation), /failed: synthetic spawn refusal/)
  assert.match(message(annotation), /synthetic diagnostic/)
})
