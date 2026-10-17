#!/usr/bin/env node
/** Bounded loaded two-device repetitions, using the matrix's existing Cargo build. */
import { createHash } from "node:crypto"
import { once } from "node:events"
import { createReadStream, createWriteStream, mkdirSync, writeFileSync } from "node:fs"
import path from "node:path"
import { finished } from "node:stream/promises"
import { pathToFileURL } from "node:url"
import { Worker } from "node:worker_threads"
import {
  buildDiagnostics,
  partitionExecutables,
  spawnProcess,
  testExecutables,
} from "../cargo-test-parallel.mjs"

export const FIXTURE =
  "composition::read_only_online_tests::online_two_devices_follow_live_hints_and_converge"
const REPETITIONS = 10
const LOAD_WORKERS = 4

export function matchedSinglePass(output) {
  return /test result: ok\. 1 passed; 0 failed;/.test(output)
}

export function runsPassed(runs, repetitions) {
  return (
    runs.length === repetitions &&
    runs.every((run) => run.exit_code === 0 && run.matched === true)
  )
}

/** A failure stays in the ledger even if every later independent run succeeds. */
export async function repeatLoaded({
  repetitions = REPETITIONS,
  startLoad,
  runTest,
  save,
}) {
  if (!Number.isInteger(repetitions) || repetitions < 1 || repetitions > REPETITIONS)
    throw new Error("repetitions must be between 1 and 10")
  const stop = await startLoad()
  const runs = []
  try {
    for (let run = 1; run <= repetitions; run += 1) {
      let result
      try {
        result = await runTest(run)
      } catch (error) {
        result = { exit_code: 1, matched: false, error: error.message }
      }
      runs.push({ run, ...result })
      await save(runs)
    }
    return runsPassed(runs, repetitions)
  } finally {
    await stop()
  }
}

async function startLoad() {
  const workers = []
  const stop = () => Promise.all(workers.map((worker) => worker.terminate()))
  try {
    const ready = []
    for (let count = 0; count < LOAD_WORKERS; count += 1) {
      const worker = new Worker(
        `const { parentPort } = require("node:worker_threads");
         parentPort.postMessage("ready");
         let value = 1; for (;;) value = Math.imul(value + 1, 1664525);`,
        { eval: true },
      )
      workers.push(worker)
      ready.push(once(worker, "message"))
    }
    await Promise.all(ready)
    return stop
  } catch (error) {
    await stop()
    throw error
  }
}

async function capture(command, args) {
  let stdout = ""
  const code = await spawnProcess(command, args, {
    cwd: process.cwd(),
    onStdout: (chunk) => (stdout += chunk),
    onStderr: (chunk) => process.stderr.write(chunk),
  })
  if (code !== 0) {
    if (args.includes("--message-format=json"))
      process.stderr.write(buildDiagnostics(stdout))
    throw new Error(`${command} exited ${code}`)
  }
  return stdout
}

async function runTest(executable, directory, run) {
  const name = `run-${String(run).padStart(2, "0")}.log`
  const log = createWriteStream(path.join(directory, name), { flags: "w" })
  const written = finished(log)
  // Observe a write error immediately; rethrow it after closing the stream.
  written.catch(() => {})
  let tail = ""
  let matched = false
  const record = (chunk) => {
    log.write(chunk)
    tail = (tail + chunk).slice(-8192)
    matched ||= matchedSinglePass(tail)
  }
  const start = performance.now()
  let exit_code
  try {
    exit_code = await spawnProcess(
      executable,
      ["--exact", FIXTURE, "--nocapture", "--test-threads=1"],
      { cwd: process.cwd(), onStdout: record, onStderr: record },
    )
  } finally {
    log.end()
    await written
  }
  const seconds = (performance.now() - start) / 1000
  process.stderr.write(
    `loaded two-device run ${run}/${REPETITIONS}: exit=${exit_code}, ${seconds.toFixed(2)}s\n`,
  )
  return { exit_code, matched, seconds, log: name }
}

async function main(args) {
  if (args[0] !== "--" || args.length < 2)
    throw new Error(
      "usage: paired-reader-load.mjs -- <same Cargo package arguments as combined build>",
    )
  const directory = path.resolve("target/paired-reader-macos-load")
  mkdirSync(directory, { recursive: true })
  const build = await capture("cargo", [
    "test",
    "--no-run",
    "--message-format=json",
    ...args.slice(1),
  ])
  process.stderr.write(buildDiagnostics(build))
  const metadata = JSON.parse(
    await capture("cargo", ["metadata", "--no-deps", "--format-version=1"]),
  )
  const [server] = partitionExecutables(testExecutables(build), metadata)["server-lib"]
  const digest = createHash("sha256")
  for await (const chunk of createReadStream(server.executable)) digest.update(chunk)
  const checkout = (await capture("git", ["rev-parse", "HEAD"])).trim()
  const identity = {
    head: process.env.PAIRED_READER_HEAD_SHA || checkout,
    checkout,
    platform: process.platform,
    arch: process.arch,
    fixture: FIXTURE,
    binary_sha256: digest.digest("hex"),
    workers: LOAD_WORKERS,
    repetitions: REPETITIONS,
  }
  const passed = await repeatLoaded({
    startLoad,
    runTest: (run) => runTest(server.executable, directory, run),
    save: (runs) => {
      const complete = runs.length === identity.repetitions
      const passed = runsPassed(runs, identity.repetitions)
      writeFileSync(
        path.join(directory, "summary.json"),
        `${JSON.stringify({ ...identity, complete, passed, runs }, null, 2)}\n`,
      )
    },
  })
  process.stdout.write(`${JSON.stringify({ ...identity, passed })}\n`)
  if (!passed) process.exitCode = 1
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    await main(process.argv.slice(2))
  } catch (error) {
    process.stderr.write(`${error.stack}\n`)
    const directory = path.resolve("target/paired-reader-macos-load")
    mkdirSync(directory, { recursive: true })
    writeFileSync(path.join(directory, "setup-error.log"), `${error.stack}\n`)
    process.exitCode = 1
  }
}
