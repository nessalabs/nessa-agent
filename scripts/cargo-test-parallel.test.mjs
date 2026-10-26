import assert from "node:assert/strict"
import { Writable } from "node:stream"
import test from "node:test"

import {
  buildDiagnostics,
  linePrefix,
  parseArgs,
  partitionExecutables,
  runParallelCargoTests,
  testExecutables,
} from "./cargo-test-parallel.mjs"

import {
  catalogueCases,
  cataloguePrefix,
  serverCasePlan,
  testRegistry,
  validateFilteredRegistry,
} from "./cargo-test-parallel/server-cases.mjs"

const registryOutput = (names) =>
  names.map((name) => `${name}: test`).join("\n") +
  `\n\n${names.length} tests, 0 benchmarks\n`
const ordinaryCases = ["ordinary::ignored", "ordinary::assertion"]
const allServerCases = [...ordinaryCases, ...catalogueCases]

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
    {
      label: "nessa-sdk nessa_sdk",
      executable: "/repo/target/debug/deps/nessa_sdk-abc",
      packageId: "path+file:///repo/crates/nessa-sdk#0.1.0",
      targetName: "nessa_sdk",
      targetKinds: ["lib"],
    },
    {
      label: "nessa-sdk infrastructure",
      executable: "/repo/target/debug/deps/infrastructure-def",
      packageId: "path+file:///repo/crates/nessa-sdk#0.1.0",
      targetName: "infrastructure",
      targetKinds: ["test"],
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

test("a multi-format native library keeps its actual unit-test executable", () => {
  const output = artifact({
    manifest: "/repo/crates/nessa-device-client/Cargo.toml",
    kind: ["rlib", "cdylib", "staticlib"],
    name: "nessa_device_client",
    executable: "/repo/target/debug/deps/nessa_device_client-abc",
  })
  assert.deepEqual(testExecutables(output), [
    {
      label: "nessa-device-client nessa_device_client",
      executable: "/repo/target/debug/deps/nessa_device_client-abc",
      packageId: "path+file:///repo/crates/nessa-sdk#0.1.0",
      targetName: "nessa_device_client",
      targetKinds: ["rlib", "cdylib", "staticlib"],
    },
  ])
  assert.deepEqual(
    testExecutables(
      artifact({ kind: ["rlib", "cdylib", "staticlib"], testProfile: false }),
    ),
    [],
  )
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
    partition: "all",
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

const metadata = {
  packages: [
    {
      name: "nessa-server",
      id: "server-id",
      targets: [{ name: "nessa_server", kind: ["lib"] }],
    },
  ],
}
function splitArtifacts() {
  const server = JSON.parse(
    artifact({
      manifest: "C:\\repo\\crates\\nessa-server\\Cargo.toml",
      name: "nessa_server",
      executable: "C:\\repo\\target\\unpredictable.exe",
    }),
  )
  server.package_id = "server-id"
  return [server, JSON.parse(artifact({ name: "nessa_server", kind: ["test"] }))]
    .map((item) => JSON.stringify(item))
    .join("\n")
}

test("Cargo identity partitions exhaustively and disjointly, including Windows paths and colliding names", () => {
  const all = testExecutables(splitArtifacts())
  const split = partitionExecutables(all, metadata)
  assert.equal(split["server-lib"].length, 1)
  assert.equal(split.remaining.length, 1)
  assert.equal(split["server-lib"][0].executable, "C:\\repo\\target\\unpredictable.exe")
  assert.deepEqual(new Set([...split["server-lib"], ...split.remaining]), new Set(all))
  assert.ok(split.remaining.every((item) => !split["server-lib"].includes(item)))
  for (const candidates of [
    [],
    split.remaining,
    [...all, { ...split["server-lib"][0], executable: "other.exe" }],
  ])
    assert.throws(
      () => partitionExecutables(candidates, metadata),
      /expected one.*executable/,
    )
  for (const packages of [[], [...metadata.packages, ...metadata.packages]])
    assert.throws(
      () => partitionExecutables(all, { packages }),
      /expected one nessa-server package/,
    )
  for (const targets of [
    [],
    [{ name: "nessa_server", kind: ["bin"] }],
    [...metadata.packages[0].targets, ...metadata.packages[0].targets],
  ])
    assert.throws(
      () =>
        partitionExecutables(all, { packages: [{ ...metadata.packages[0], targets }] }),
      /expected one.*library target/,
    )
})

test("partition arguments reject misspellings before building", () => {
  assert.equal(
    parseArgs([
      "--concurrency",
      "2",
      "--partition",
      "server-lib",
      "--",
      "-p",
      "nessa-server",
    ]).partition,
    "server-lib",
  )
  assert.throws(
    () =>
      parseArgs([
        "--concurrency",
        "2",
        "--partition",
        "other",
        "--",
        "-p",
        "nessa-server",
      ]),
    /unknown test partition/,
  )
})

test("both split modes build the full selection, and remaining alone owns doctests", async () => {
  for (const partition of ["server-lib", "remaining"]) {
    const calls = []
    const io = streams()
    await runParallelCargoTests({
      cargoArgs: packages,
      concurrency: 2,
      partition,
      ...io,
      async run(command, args, invocation) {
        calls.push({ command, args })
        if (args.includes("--no-run")) invocation.onStdout(splitArtifacts())
        if (args[0] === "metadata") invocation.onStdout(JSON.stringify(metadata))
        if (args.includes("--list")) {
          const names = args.includes("--skip")
            ? ordinaryCases
            : args.includes(cataloguePrefix)
              ? catalogueCases
              : allServerCases
          invocation.onStdout(registryOutput(names))
        }
        return 0
      },
    })
    assert.deepEqual(calls[0].args, [
      "test",
      "--no-run",
      "--message-format=json",
      ...packages,
    ])
    assert.equal(
      calls.filter((call) => call.args.includes("--doc")).length,
      partition === "remaining" ? 1 : 0,
    )
    assert.equal(calls.filter((call) => call.args.includes("--list")).length, 3)
    const executed = calls.filter(
      (call) => call.command !== "cargo" && !call.args.includes("--list"),
    )
    assert.equal(executed.length, partition === "remaining" ? 2 : 1)
    assert.deepEqual(
      executed[0].args,
      partition === "remaining" ? [cataloguePrefix] : ["--skip", cataloguePrefix],
    )
  }
})

test("split discovery failures start no executable or doctest", async () => {
  for (const mode of ["metadata-failure", "bad-json", "missing-library"]) {
    const calls = []
    const io = streams()
    await assert.rejects(
      runParallelCargoTests({
        cargoArgs: packages,
        concurrency: 2,
        partition: "remaining",
        ...io,
        async run(command, args, invocation) {
          calls.push(command)
          if (args.includes("--no-run"))
            invocation.onStdout(
              mode === "missing-library" ? artifact({}) : splitArtifacts(),
            )
          if (args[0] === "metadata") {
            invocation.onStdout(mode === "bad-json" ? "{" : JSON.stringify(metadata))
            return mode === "metadata-failure" ? 1 : 0
          }
          return 0
        },
      }),
    )
    assert.deepEqual(calls, ["cargo", "cargo"])
  }
})

test("each part of Cargo identity excludes an otherwise matching executable", () => {
  const server = testExecutables(splitArtifacts())[0]
  const nearMatches = [
    { ...server, executable: "another-package.exe", packageId: "another-package" },
    { ...server, executable: "another-target.exe", targetName: "another_target" },
    { ...server, executable: "binary-target.exe", targetKinds: ["bin"] },
  ]
  const split = partitionExecutables([server, ...nearMatches], metadata)
  assert.deepEqual(split["server-lib"], [server])
  assert.deepEqual(split.remaining, nearMatches)
})

test("server case assignment is exhaustive/disjoint and registry failures cannot turn into green zero tests", () => {
  const registry = testRegistry(registryOutput(allServerCases))
  const plan = serverCasePlan(registry)
  assert.deepEqual(
    [...plan["server-lib"].names, ...plan.remaining.names].sort(),
    registry,
  )
  assert.equal(
    new Set([...plan["server-lib"].names, ...plan.remaining.names]).size,
    registry.length,
  )
  for (const invalid of [
    "",
    registryOutput(allServerCases).replace(/6 tests/, "7 tests"),
    registryOutput([...allServerCases, allServerCases[0]]),
    registryOutput(allServerCases) + "trailing noise",
    "ordinary: benchmark\n\n0 tests, 1 benchmark",
  ])
    assert.throws(() => testRegistry(invalid))
  for (const names of [
    ordinaryCases,
    allServerCases.slice(0, -1),
    [...allServerCases, `${cataloguePrefix}renamed`],
  ])
    assert.throws(() => serverCasePlan(testRegistry(registryOutput(names))))
  assert.throws(() => serverCasePlan([...catalogueCases].sort()), /no ordinary/)

  for (const names of [[], ordinaryCases, allServerCases])
    assert.throws(() =>
      validateFilteredRegistry(registryOutput(names), plan.remaining.names),
    )
})

test("registry and actual filter failures prevent all execution and doctests in either split", async () => {
  for (const partition of ["remaining", "server-lib"])
    for (const mode of [
      "exit",
      "missing-seed",
      "malformed",
      "empty-positive",
      "unfiltered-negative",
    ]) {
      const executed = []
      await assert.rejects(
        runParallelCargoTests({
          cargoArgs: packages,
          concurrency: 2,
          partition,
          ...streams(),
          async run(command, args, io) {
            if (args.includes("--no-run")) io.onStdout(splitArtifacts())
            else if (args[0] === "metadata") io.onStdout(JSON.stringify(metadata))
            else if (args.includes("--list")) {
              if (mode === "exit") return 1
              const names = args.includes("--skip")
                ? mode === "unfiltered-negative"
                  ? allServerCases
                  : ordinaryCases
                : args.includes(cataloguePrefix)
                  ? mode === "empty-positive"
                    ? []
                    : catalogueCases
                  : mode === "missing-seed"
                    ? allServerCases.slice(0, -1)
                    : allServerCases
              io.onStdout(mode === "malformed" ? "truncated" : registryOutput(names))
            } else executed.push({ command, args })
            return 0
          },
        }),
      )
      assert.deepEqual(executed, [], `${partition} ${mode}`)
    }
})
