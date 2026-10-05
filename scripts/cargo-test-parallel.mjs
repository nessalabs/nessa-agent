#!/usr/bin/env node
/**
 * Run one combined `cargo test` build, then its test binaries two at a time.
 *
 * `cargo test` already compiles dependencies concurrently. It still runs each
 * test binary to completion before it starts the next, which is most of the
 * Windows matrix leg. This keeps that single package selection — feature
 * unification and all — and overlaps the binaries cargo would have run in order.
 *
 * libtest's thread count is left alone. The ACP process-contract tests share a
 * process-local two-slot semaphore (`PROCESS_TEST_SLOTS` in
 * `crates/nessa-sdk/tests/infrastructure/acp/contracts/support.rs`). That bound
 * holds only while those tests share a process. cargo-nextest runs each test in
 * its own process, so the semaphore would admit every contract test at once,
 * and doctests would need a second mechanism. Doctests are not part of
 * `--no-run`'s executable list; they run here with `cargo test --doc` after the
 * binaries, once cargo is no longer writing the target directory.
 *
 * A build that reports no test binaries fails. An empty list is indistinguishable
 * from a parser that understood nothing, and the run that skips the suite must
 * not be green.
 *
 *   node scripts/cargo-test-parallel.mjs --concurrency 2 -- -p nessa-sdk
 *
 * Diagnostics go to stderr. Test output goes to stdout, one binary per prefix.
 * The process exits non-zero when the build, a binary, or doctests fail.
 */
import { spawn } from "node:child_process"
import path from "node:path"
import { pathToFileURL } from "node:url"

const TEST_KINDS = new Set(["lib", "bin", "test"])
const SKIPPED_KINDS = new Set(["custom-build", "example", "bench"])

/** Split `--concurrency N -- cargo-args` from the process argument list. */
export function parseArgs(argv) {
  const separator = argv.indexOf("--")
  if (separator === -1) throw new Error("cargo arguments must follow --")
  const flags = argv.slice(0, separator)
  const cargoArgs = argv.slice(separator + 1)
  if (cargoArgs.length === 0) throw new Error("cargo test package arguments are required")
  if (flags.length !== 2 || flags[0] !== "--concurrency")
    throw new Error("usage: cargo-test-parallel.mjs --concurrency N -- <cargo test args>")
  const concurrency = Number(flags[1])
  if (!Number.isInteger(concurrency) || concurrency < 1)
    throw new Error("concurrency must be a positive integer")
  return { concurrency, cargoArgs }
}

/**
 * Test harnesses from `cargo test --no-run --message-format=json`.
 *
 * The plain binary cargo also emits (profile.test false) is the program, not
 * its tests. Doctests are absent here; the caller runs them separately.
 */
export function testExecutables(stdout) {
  const executables = []
  const seen = new Set()
  for (const line of stdout.split(/\r?\n/)) {
    if (line.trim() === "") continue
    let message
    try {
      message = JSON.parse(line)
    } catch {
      continue
    }
    if (message.reason !== "compiler-artifact") continue
    const profile = message.profile
    const executable = message.executable
    const target = message.target
    if (!profile || profile.test !== true) continue
    if (typeof executable !== "string" || executable.length === 0) continue
    if (!target || !Array.isArray(target.kind)) continue
    if (target.kind.some((kind) => SKIPPED_KINDS.has(kind))) continue
    if (!target.kind.some((kind) => TEST_KINDS.has(kind))) continue
    if (seen.has(executable)) continue
    seen.add(executable)
    const manifest =
      typeof message.manifest_path === "string" ? message.manifest_path : ""
    const crate = path.basename(path.dirname(manifest))
    const name = typeof target.name === "string" ? target.name : path.basename(executable)
    executables.push({ label: `${crate} ${name}`, executable })
  }
  return executables
}

/** Compiler diagnostics buried in `cargo --message-format=json` stdout. */
export function buildDiagnostics(stdout) {
  const rendered = []
  for (const line of stdout.split(/\r?\n/)) {
    if (line.trim() === "") continue
    let message
    try {
      message = JSON.parse(line)
    } catch {
      rendered.push(`${line}\n`)
      continue
    }
    if (
      message.reason === "compiler-message" &&
      typeof message.message?.rendered === "string"
    )
      rendered.push(message.message.rendered)
  }
  return rendered.join("")
}

/** Prefix each complete line. A chunk may end mid-line. */
export function linePrefix(label, write) {
  let pending = ""
  return {
    push(chunk) {
      pending += chunk
      const lines = pending.split("\n")
      pending = lines.pop()
      for (const line of lines) write(`[${label}] ${line}\n`)
    },
    end() {
      if (pending.length > 0) write(`[${label}] ${pending}\n`)
      pending = ""
    },
  }
}

async function runPool(tasks, concurrency, runTask) {
  const failures = []
  let index = 0
  let stop = false
  async function worker() {
    for (;;) {
      if (stop) return
      if (index >= tasks.length) return
      const task = tasks[index]
      index += 1
      try {
        await runTask(task)
      } catch (error) {
        failures.push(error)
        stop = true
      }
    }
  }
  const workers = Math.min(concurrency, tasks.length)
  await Promise.all(Array.from({ length: workers }, () => worker()))
  return failures
}

function secondsSince(start) {
  return ((Date.now() - start) / 1000).toFixed(1)
}

/**
 * Build once, run harnesses with the given concurrency, then run doctests.
 *
 * `run(command, args, io)` returns an exit code. `io.onStdout` and `io.onStderr`
 * receive strings. A non-zero build or an empty harness list throws before any
 * harness starts.
 */
export async function runParallelCargoTests({
  cargoArgs,
  concurrency,
  run,
  cwd,
  stdout = process.stdout,
  stderr = process.stderr,
}) {
  if (!Array.isArray(cargoArgs) || cargoArgs.length === 0)
    throw new Error("cargo test package arguments are required")
  if (!Number.isInteger(concurrency) || concurrency < 1)
    throw new Error("concurrency must be a positive integer")

  const buildStarted = Date.now()
  let buildStdout = ""
  const buildCode = await run(
    "cargo",
    ["test", "--no-run", "--message-format=json", ...cargoArgs],
    {
      cwd,
      onStdout: (chunk) => {
        buildStdout += chunk
      },
      onStderr: (chunk) => stderr.write(chunk),
    },
  )
  const diagnostics = buildDiagnostics(buildStdout)
  if (diagnostics.length > 0)
    stderr.write(diagnostics.endsWith("\n") ? diagnostics : `${diagnostics}\n`)
  if (buildCode !== 0) throw new Error(`cargo test --no-run exited with ${buildCode}`)
  const executables = testExecutables(buildStdout)
  if (executables.length === 0)
    throw new Error("cargo test --no-run produced no test binaries")
  stderr.write(
    `test build finished in ${secondsSince(buildStarted)}s and produced ${executables.length} binaries\n`,
  )

  const executionStarted = Date.now()
  const failures = await runPool(
    executables,
    concurrency,
    async ({ label, executable }) => {
      stderr.write(`starting ${label}\n`)
      const started = Date.now()
      const prefixed = linePrefix(label, (line) => stdout.write(line))
      const code = await run(executable, [], {
        cwd,
        onStdout: (chunk) => prefixed.push(chunk),
        onStderr: (chunk) => prefixed.push(chunk),
      })
      prefixed.end()
      stderr.write(`${label} finished in ${secondsSince(started)}s with exit ${code}\n`)
      if (code !== 0) throw new Error(`${label} exited with ${code}`)
    },
  )
  if (failures.length > 0) {
    throw new Error(failures.map((failure) => failure.message).join("\n"))
  }
  stderr.write(
    `test execution finished in ${secondsSince(executionStarted)}s with concurrency ${concurrency}\n`,
  )

  const doctests = linePrefix("doctests", (line) => stdout.write(line))
  const doctestCode = await run("cargo", ["test", "--doc", ...cargoArgs], {
    cwd,
    onStdout: (chunk) => doctests.push(chunk),
    onStderr: (chunk) => doctests.push(chunk),
  })
  doctests.end()
  if (doctestCode !== 0) throw new Error(`cargo test --doc exited with ${doctestCode}`)
}

function spawnProcess(command, args, io) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: io.cwd,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    })
    child.stdout.setEncoding("utf8")
    child.stderr.setEncoding("utf8")
    child.stdout.on("data", (chunk) => io.onStdout?.(chunk))
    child.stderr.on("data", (chunk) => io.onStderr?.(chunk))
    child.on("error", reject)
    child.on("close", (code) => resolve(code ?? 1))
  })
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) {
  try {
    const { concurrency, cargoArgs } = parseArgs(process.argv.slice(2))
    await runParallelCargoTests({
      cargoArgs,
      concurrency,
      run: spawnProcess,
      cwd: process.cwd(),
    })
  } catch (error) {
    process.stderr.write(`${error.message}\n`)
    process.exitCode = 1
  }
}
