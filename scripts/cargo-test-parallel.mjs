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
 * binaries, once cargo is no longer writing the target directory. Windows may
 * select server-conversation, server-other or remaining after the same full build; remaining owns
 * doctests. See cargo-test-parallel/README.md for the partition contract.
 *
 * A build that reports no test binaries fails. An empty list is indistinguishable
 * from a parser that understood nothing, and the run that skips the suite must
 * not be green.
 *
 * `--run-packages "-p a -p b"` runs only the binaries of those packages. The
 * build and the doctests still take the full package arguments, so a narrowed
 * run compiles exactly what the full one does and only runs less of it. An
 * empty value runs every binary. `scripts/ci-plan.mjs` decides the list.
 *
 *   node scripts/cargo-test-parallel.mjs --concurrency 2 -- -p nessa-sdk
 *
 * Diagnostics go to stderr. Test output goes to stdout, one binary per prefix.
 * The process exits non-zero when the build, a binary, or doctests fail.
 */
import { spawn } from "node:child_process"
import path from "node:path"
import { pathToFileURL } from "node:url"
import { withFailureAnnotations } from "./cargo-test-parallel/diagnostics.mjs"
import {
  serverCasePlan,
  testRegistry,
  validateFilteredRegistry,
} from "./cargo-test-parallel/server-cases.mjs"

const PARTITIONS = ["all", "server-conversation", "server-other", "remaining"]

const TEST_KINDS = new Set(["lib", "rlib", "cdylib", "staticlib", "bin", "test"])
const SKIPPED_KINDS = new Set(["custom-build", "example", "bench"])

const USAGE =
  "usage: cargo-test-parallel.mjs --concurrency N [--partition all|server-conversation|server-other|remaining] [--run-packages '-p a -p b'] -- <cargo test args>"

/** Split `--concurrency N [...] -- cargo-args` from the process argument list. */
export function parseArgs(argv) {
  const separator = argv.indexOf("--")
  if (separator === -1) throw new Error("cargo arguments must follow --")
  const flags = argv.slice(0, separator)
  const cargoArgs = argv.slice(separator + 1)
  if (cargoArgs.length === 0) throw new Error("cargo test package arguments are required")
  const values = new Map()
  for (let index = 0; index < flags.length; index += 2) {
    const flag = flags[index]
    if (
      !["--concurrency", "--partition", "--run-packages"].includes(flag) ||
      values.has(flag) ||
      index + 1 >= flags.length
    )
      throw new Error(USAGE)
    values.set(flag, flags[index + 1])
  }
  if (!values.has("--concurrency") || flags[0] !== "--concurrency") throw new Error(USAGE)
  const concurrency = Number(values.get("--concurrency"))
  if (!Number.isInteger(concurrency) || concurrency < 1)
    throw new Error("concurrency must be a positive integer")
  const partition = values.get("--partition") ?? "all"
  if (!PARTITIONS.includes(partition)) throw new Error("unknown test partition")
  const runPackages = runPackageNames(values.get("--run-packages") ?? "")
  return runPackages.length > 0
    ? { concurrency, cargoArgs, partition, runPackages }
    : { concurrency, cargoArgs, partition }
}

/** The names in a `-p a -p b` string; empty means every package. */
export function runPackageNames(text) {
  const words = text.split(/\s+/).filter((word) => word !== "")
  const names = []
  for (let index = 0; index < words.length; index += 2) {
    if (words[index] !== "-p" || !words[index + 1])
      throw new Error(`--run-packages takes "-p name" pairs, not ${JSON.stringify(text)}`)
    names.push(words[index + 1])
  }
  return names
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
    executables.push({
      label: `${crate} ${name}`,
      executable,
      packageId: message.package_id,
      targetName: target.name,
      targetKinds: target.kind,
    })
  }
  return executables
}

/** Resolve the server library by Cargo package/target identity, then partition once.
 * partition tests assert the union and intersection, including Windows paths.
 */
export function partitionExecutables(executables, metadata) {
  const packages = metadata.packages.filter((pkg) => pkg.name === "nessa-server")
  if (packages.length !== 1) throw new Error("expected one nessa-server package")
  const server = packages[0]
  const libraries = server.targets.filter((target) => target.kind.includes("lib"))
  if (libraries.length !== 1) throw new Error("expected one nessa-server library target")
  const library = libraries[0]
  const serverLib = executables.filter(
    (item) =>
      item.packageId === server.id &&
      item.targetName === library.name &&
      item.targetKinds.includes("lib"),
  )
  if (serverLib.length !== 1)
    throw new Error(
      `expected one nessa-server library test executable, found ${serverLib.length}`,
    )
  const remaining = executables.filter((item) => item !== serverLib[0])
  return { server: serverLib, remaining }
}

/**
 * The executables that belong to the named packages. A name the workspace does
 * not have, or a selection with nothing in it, is an error: a run that tests
 * nothing must not pass.
 */
export function selectPackages(executables, metadata, names) {
  const ids = new Map(metadata.packages.map((pkg) => [pkg.name, pkg.id]))
  for (const name of names)
    if (!ids.has(name)) throw new Error(`no package named ${name} in this workspace`)
  const wanted = new Set(names.map((name) => ids.get(name)))
  const selected = executables.filter((item) => wanted.has(item.packageId))
  if (selected.length === 0)
    throw new Error(`no test binaries belong to ${names.join(", ")}`)
  return selected
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
  partition = "all",
  runPackages = [],
  run,
  cwd,
  stdout = process.stdout,
  stderr = process.stderr,
}) {
  if (!Array.isArray(cargoArgs) || cargoArgs.length === 0)
    throw new Error("cargo test package arguments are required")
  if (!Number.isInteger(concurrency) || concurrency < 1)
    throw new Error("concurrency must be a positive integer")

  if (!PARTITIONS.includes(partition)) throw new Error("unknown test partition")

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

  let selected = executables
  let metadata = null
  if (partition !== "all" || runPackages.length > 0) {
    let text = ""
    const metadataCode = await run(
      "cargo",
      ["metadata", "--no-deps", "--format-version=1"],
      {
        cwd,
        onStdout: (chunk) => {
          text += chunk
        },
        onStderr: (chunk) => stderr.write(chunk),
      },
    )
    if (metadataCode !== 0) throw new Error(`cargo metadata exited with ${metadataCode}`)
    metadata = JSON.parse(text)
  }
  if (partition !== "all") {
    const partitions = partitionExecutables(executables, metadata)
    const server = partitions.server[0]
    async function list(args) {
      let output = ""
      const code = await run(server.executable, [...args, "--list"], {
        cwd,
        onStdout: (chunk) => {
          output += chunk
        },
        onStderr: (chunk) => stderr.write(chunk),
      })
      if (code !== 0) throw new Error(`server libtest registry exited with ${code}`)
      return output
    }
    const plan = serverCasePlan(testRegistry(await list([])))
    for (const part of ["server-conversation", "server-other", "remaining"])
      for (const task of plan[part].tasks)
        validateFilteredRegistry(await list(task.args), task.names)
    const serverTasks = plan[partition].tasks.map((task) => ({
      ...server,
      label: `${server.label} ${task.label}`,
      args: task.args,
    }))
    selected =
      partition === "remaining" ? [...serverTasks, ...partitions.remaining] : serverTasks
    stderr.write(
      `partition ${partition}: ${plan[partition].names.length} server cases in ${serverTasks.length} executables, ${selected.length - serverTasks.length} other binaries; doctests ${partition === "remaining" ? "included" : "owned by remaining"}\n`,
    )
  }

  if (runPackages.length > 0) {
    selected = selectPackages(selected, metadata, runPackages)
    stderr.write(
      `running the ${selected.length} binaries of ${runPackages.join(", ")}; doctests keep the full list\n`,
    )
  }

  const executionStarted = Date.now()
  const failures = await runPool(
    selected,
    concurrency,
    async ({ label, executable, args = [] }) => {
      stderr.write(`starting ${label}\n`)
      const started = Date.now()
      const prefixed = linePrefix(label, (line) => stdout.write(line))
      const code = await run(executable, args, {
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

  if (partition !== "all" && partition !== "remaining") return

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
    const { concurrency, cargoArgs, partition, runPackages } = parseArgs(
      process.argv.slice(2),
    )
    await runParallelCargoTests({
      cargoArgs,
      concurrency,
      partition,
      runPackages,
      run:
        process.env.GITHUB_ACTIONS === "true"
          ? withFailureAnnotations(spawnProcess, (line) => process.stderr.write(line))
          : spawnProcess,
      cwd: process.cwd(),
    })
  } catch (error) {
    process.stderr.write(`${error.message}\n`)
    process.exitCode = 1
  }
}
