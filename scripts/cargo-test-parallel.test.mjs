import assert from "node:assert/strict"
import { Writable } from "node:stream"
import test from "node:test"

import {
  buildDiagnostics,
  linePrefix,
  parseArgs,
  runParallelCargoTests,
  testExecutables,
} from "./cargo-test-parallel.mjs"

function artifact({
  manifest = "/repo/crates/nessa-sdk/Cargo.toml",
  kind = ["lib"],
  name = "nessa_sdk",
  testProfile = true,
  executable = "/repo/target/debug/deps/nessa_sdk-abc",
}) {
  return JSON.stringify({
    reason: "compiler-artifact",
    package_id: "path+file:///repo/crates/nessa-sdk#0.1.0",
    manifest_path: manifest,
    target: { kind, name, doctest: true },
    profile: { test: testProfile },
    executable,
  })
}

function streams() {
  let out = ""
  let err = ""
  const stdout = new Writable({
    write(chunk, _encoding, callback) {
      out += chunk.toString()
      callback()
    },
  })
  const stderr = new Writable({
    write(chunk, _encoding, callback) {
      err += chunk.toString()
      callback()
    },
  })
  return {
    stdout,
    stderr,
    text: () => ({ out, err }),
  }
}

test("the harness list keeps test binaries and drops the plain program", () => {
  const stdout = [
    artifact({
      kind: ["bin"],
      name: "nessa",
      testProfile: false,
      executable: "/repo/target/debug/nessa",
    }),
    artifact({ executable: null }),
    artifact({}),
    artifact({
      manifest: "/repo/crates/nessa-sdk/Cargo.toml",
      kind: ["test"],
      name: "infrastructure",
      executable: "/repo/target/debug/deps/infrastructure-def",
    }),
    artifact({}),
    JSON.stringify({
      reason: "compiler-message",
      message: { rendered: "warning: unused\n" },
    }),
    "not json",
  ].join("\n")

  assert.deepEqual(testExecutables(stdout), [
    { label: "nessa-sdk nessa_sdk", executable: "/repo/target/debug/deps/nessa_sdk-abc" },
    {
      label: "nessa-sdk infrastructure",
      executable: "/repo/target/debug/deps/infrastructure-def",
    },
  ])
  assert.equal(buildDiagnostics(stdout), "warning: unused\nnot json\n")
})

test("examples, benches, and build scripts are not test binaries", () => {
  const stdout = [
    artifact({
      kind: ["example"],
      name: "demo",
      executable: "/repo/target/debug/examples/demo",
    }),
    artifact({
      kind: ["bench"],
      name: "perf",
      executable: "/repo/target/debug/deps/perf-a",
    }),
    artifact({
      kind: ["custom-build"],
      name: "build-script-build",
      executable: "/repo/target/debug/build/build-script-build",
    }),
  ].join("\n")
  assert.deepEqual(testExecutables(stdout), [])
})

test("a line prefix keeps a fragment that arrives split", () => {
  let written = ""
  const prefix = linePrefix("infrastructure", (line) => {
    written += line
  })
  prefix.push("hello ")
  prefix.push("world\nnext")
  prefix.end()
  assert.equal(written, "[infrastructure] hello world\n[infrastructure] next\n")
})

test("the command requires a positive concurrency and cargo arguments", () => {
  assert.deepEqual(parseArgs(["--concurrency", "2", "--", "-p", "nessa-sdk"]), {
    concurrency: 2,
    cargoArgs: ["-p", "nessa-sdk"],
  })
  assert.throws(() => parseArgs(["-p", "nessa-sdk"]), /must follow --/)
  assert.throws(
    () => parseArgs(["--concurrency", "2", "--"]),
    /package arguments are required/,
  )
  assert.throws(
    () => parseArgs(["--concurrency", "0", "--", "-p", "nessa-sdk"]),
    /positive integer/,
  )
  assert.throws(
    () => parseArgs(["--jobs", "2", "--", "-p", "nessa-sdk"]),
    /usage: cargo-test-parallel/,
  )
})

const packages = ["-p", "nessa-local-storage", "-p", "nessa-sdk"]

function harnessJson() {
  return [
    artifact({}),
    artifact({
      kind: ["test"],
      name: "infrastructure",
      executable: "/repo/target/debug/deps/infrastructure-def",
    }),
    artifact({
      manifest: "/repo/crates/nessa-server/Cargo.toml",
      kind: ["lib"],
      name: "nessa_server",
      executable: "/repo/target/debug/deps/nessa_server-ghi",
    }),
  ].join("\n")
}

test("one build uses the caller's packages, and each harness keeps libtest's default threads", async () => {
  const calls = []
  const io = streams()
  await runParallelCargoTests({
    cargoArgs: packages,
    concurrency: 2,
    cwd: "/repo",
    stdout: io.stdout,
    stderr: io.stderr,
    async run(command, args, invocation) {
      calls.push({ command, args, cwd: invocation.cwd })
      if (args.includes("--no-run")) invocation.onStdout?.(`${harnessJson()}\n`)
      invocation.onStdout?.("ran\n")
      return 0
    },
  })

  assert.deepEqual(calls[0], {
    command: "cargo",
    args: ["test", "--no-run", "--message-format=json", ...packages],
    cwd: "/repo",
  })
  const binaries = calls.slice(1, -1)
  assert.deepEqual(
    binaries.map((call) => call.command),
    [
      "/repo/target/debug/deps/nessa_sdk-abc",
      "/repo/target/debug/deps/infrastructure-def",
      "/repo/target/debug/deps/nessa_server-ghi",
    ],
  )
  for (const call of binaries) assert.deepEqual(call.args, [])
  assert.deepEqual(calls.at(-1), {
    command: "cargo",
    args: ["test", "--doc", ...packages],
    cwd: "/repo",
  })
  assert.match(io.text().out, /\[nessa-sdk nessa_sdk\] ran\n/)
  assert.match(io.text().err, /produced 3 binaries/)
  assert.match(io.text().err, /with concurrency 2/)
})

test("two harnesses run at a time, and a failure does not start the rest or the doctests", async () => {
  const started = []
  let active = 0
  let max = 0
  const io = streams()
  await assert.rejects(
    runParallelCargoTests({
      cargoArgs: packages,
      concurrency: 2,
      stdout: io.stdout,
      stderr: io.stderr,
      async run(command, args, invocation) {
        if (args.includes("--no-run")) {
          invocation.onStdout?.(`${harnessJson()}\n`)
          return 0
        }
        if (args.includes("--doc"))
          throw new Error("doctests started after a harness failure")
        started.push(command)
        active += 1
        max = Math.max(max, active)
        active -= 1
        return command.endsWith("nessa_sdk-abc") ? 1 : 0
      },
    }),
    /nessa-sdk nessa_sdk exited with 1/,
  )
  assert.equal(max, 1)
  assert.deepEqual(started, [
    "/repo/target/debug/deps/nessa_sdk-abc",
    "/repo/target/debug/deps/infrastructure-def",
  ])
})

test("a passing suite still fails when doctests fail", async () => {
  const io = streams()
  await assert.rejects(
    runParallelCargoTests({
      cargoArgs: ["-p", "nessa-sdk"],
      concurrency: 2,
      stdout: io.stdout,
      stderr: io.stderr,
      async run(_command, args, invocation) {
        if (args.includes("--no-run")) invocation.onStdout?.(artifact({}))
        if (args.includes("--doc")) return 1
        return 0
      },
    }),
    /cargo test --doc exited with 1/,
  )
})

test("a failed build or a build with no harnesses does not run tests", async () => {
  const started = []
  const io = streams()
  await assert.rejects(
    runParallelCargoTests({
      cargoArgs: packages,
      concurrency: 2,
      stdout: io.stdout,
      stderr: io.stderr,
      async run(command, args, invocation) {
        started.push(args[0] ?? command)
        if (args.includes("--no-run")) {
          invocation.onStdout?.(
            `${JSON.stringify({
              reason: "compiler-message",
              message: { rendered: "error: could not compile\n" },
            })}\n`,
          )
          return 1
        }
        return 0
      },
    }),
    /cargo test --no-run exited with 1/,
  )
  assert.deepEqual(started, ["test"])
  assert.match(io.text().err, /could not compile/)

  started.length = 0
  await assert.rejects(
    runParallelCargoTests({
      cargoArgs: packages,
      concurrency: 2,
      stdout: io.stdout,
      stderr: io.stderr,
      async run(_command, args, invocation) {
        started.push(args.includes("--doc") ? "doc" : "build")
        if (args.includes("--no-run")) {
          invocation.onStdout?.(
            artifact({
              kind: ["bin"],
              name: "nessa",
              testProfile: false,
              executable: "/repo/target/debug/nessa",
            }),
          )
        }
        return 0
      },
    }),
    /produced no test binaries/,
  )
  assert.deepEqual(started, ["build"])
})

test("concurrency is a bound, not a thread-count increase", async () => {
  let active = 0
  let max = 0
  const waiting = []
  const io = streams()
  const four = [
    artifact({ name: "one", executable: "/bin/one" }),
    artifact({ name: "two", executable: "/bin/two" }),
    artifact({ name: "three", executable: "/bin/three" }),
    artifact({ name: "four", executable: "/bin/four" }),
  ].join("\n")
  await runParallelCargoTests({
    cargoArgs: ["-p", "nessa-sdk"],
    concurrency: 2,
    stdout: io.stdout,
    stderr: io.stderr,
    async run(_command, args, invocation) {
      if (args.includes("--no-run")) {
        invocation.onStdout?.(four)
        return 0
      }
      if (args.includes("--doc")) return 0
      active += 1
      max = Math.max(max, active)
      await new Promise((resolve) => {
        waiting.push(resolve)
        if (waiting.length === 2) {
          const batch = waiting.splice(0, waiting.length)
          for (const release of batch) release()
        }
      })
      active -= 1
      return 0
    },
  })
  assert.equal(max, 2)
  assert.doesNotMatch(io.text().err, /--test-threads/)
})
