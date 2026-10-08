import assert from "node:assert/strict"
import test from "node:test"
import { EventEmitter } from "node:events"
import { mkdtemp, mkdir, writeFile, readFile, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import {
  checkContainers,
  docker,
  DockerFailure,
  runScenario,
  publishAcceptance,
  scenarios,
  validate,
} from "./check-container.mjs"

function report(scenario) {
  return {
    test: scenario.test,
    timed_out: false,
    output_truncated: false,
    test_pid: scenario.init ? 8 : 7,
    pid1_executable: scenario.init ? "/usr/sbin/docker-init" : "/usr/bin/python3.13",
    initial_processes: scenario.init
      ? [
          { pid: 1, ppid: 0, pgid: 1, state: "S" },
          { pid: 7, ppid: 1, pgid: 7, state: "R" },
        ]
      : [{ pid: 1, ppid: 0, pgid: 1, state: "R" }],
    packages: {
      "python3-minimal": "3.13.5-1",
      "libgcc-s1": "14.2.0-19",
      "ca-certificates": "20250419",
    },
    supervisor_pid: scenario.init ? 7 : 1,
    supervisor_ppid: scenario.init ? 1 : 0,
    test_exit: scenario.init ? 0 : 101,
    output: scenario.init
      ? `running 1 test\ntest ${scenario.test} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;`
      : `running 1 test\ntest ${scenario.test} ... FAILED\nthread '${scenario.test}' (8) panicked at fixture.rs:69:10:\ncalled \`Result::unwrap()\` on an \`Err\` value: CleanupUncertain\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;`,
    new_orphans: scenario.init ? [] : [{ pid: 10, ppid: 1, pgid: 9, state: "Z" }],
    retained_directories: scenario.init ? [] : ["/tmp/nessa-agent-owned"],
  }
}

for (const scenario of scenarios) {
  test(`${scenario.init ? "init" : "negative"} ${scenario.test}`, () => {
    assert.equal(validate(report(scenario), scenario).test, scenario.test)
  })
}

test("zero selected tests cannot pass", () => {
  const positive = scenarios[1]
  assert.throws(
    () =>
      validate(
        {
          ...report(positive),
          output: `running 0 tests\ntest ${positive.test} ... ok\ntest result: ok. 0 passed; 0 failed; 0 ignored;`,
        },
        positive,
      ),
    /one test/,
  )
})
test("a different negative failure is a harness failure", () => {
  assert.throws(
    () =>
      validate(
        {
          ...report(scenarios[0]),
          output: report(scenarios[0]).output.replace("CleanupUncertain", "Deadline"),
        },
        scenarios[0],
      ),
    /CleanupUncertain/,
  )
})
test("the negative requires a new adopted zombie", () => {
  for (const new_orphans of [
    [],
    [{ pid: 10, ppid: 2, pgid: 9, state: "Z" }],
    [{ pid: 10, ppid: 1, pgid: 9, state: "S" }],
  ]) {
    assert.throws(
      () => validate({ ...report(scenarios[0]), new_orphans }, scenarios[0]),
      /zombie/,
    )
  }
})
test("directory retention is required in the negative directory case", () => {
  assert.throws(
    () => validate({ ...report(scenarios[0]), retained_directories: [] }, scenarios[0]),
    /released/,
  )
})
test("positive failures, zombies, retention and timeout cannot pass", () => {
  const positive = scenarios[1]
  for (const change of [
    { test_exit: 101 },
    { new_orphans: report(scenarios[0]).new_orphans },
    { retained_directories: ["/tmp/nessa-agent-owned"] },
    { timed_out: true },
  ]) {
    assert.throws(
      () => validate({ ...report(positive), ...change }, positive),
      /Harness failure/,
    )
  }
})

const options = {
  binary: "/binary",
  manifest: "/compiled/crate",
  fixture: "/compiled/crate/tests/infrastructure/acp/contracts/fixtures",
  image: "acceptance",
}

test("four scenarios use distinct containers, same inputs and readonly mounts", async () => {
  const creates = [],
    removes = []
  for (const scenario of scenarios) {
    await runScenario(options, scenario, async (args) => {
      if (args[0] === "create") creates.push(args)
      if (args[0] === "rm") removes.push(args)
      return args[0] === "start" ? JSON.stringify(report(scenario)) : ""
    })
  }
  assert.equal(new Set(creates.map((args) => args[2])).size, 4)
  assert.equal(removes.length, 4)
  for (const args of creates) {
    assert.ok(args.includes("--network=none"))
    assert.ok(args.includes("--read-only"))
    assert.ok(args.includes("type=bind,src=/binary,dst=/probe/nessa-sdk-tests,readonly"))
    assert.ok(
      args.includes(`type=bind,src=${options.fixture},dst=${options.fixture},readonly`),
    )
    assert.equal(args.at(-2), options.image)
  }
  assert.equal(creates.filter((args) => args.includes("--init")).length, 2)
})

for (const stage of ["create", "start", "validation", "interrupt"]) {
  test(`removal is attempted after ${stage} failure`, async () => {
    const calls = [],
      controller = new AbortController()
    const run = async (args, operation) => {
      calls.push([args, operation])
      if (args[0] === stage) throw new Error("injected docker failure")
      if (args[0] === "start" && stage === "interrupt") {
        controller.abort()
        throw new Error("interrupted")
      }
      if (args[0] === "start")
        return JSON.stringify(
          stage === "validation"
            ? { ...report(scenarios[0]), timed_out: true }
            : report(scenarios[0]),
        )
      return ""
    }
    await assert.rejects(
      runScenario({ ...options, signal: controller.signal }, scenarios[0], run),
    )
    assert.equal(calls.at(-1)[0][0], "rm")
    assert.equal(calls.at(-1)[1].signal, undefined)
    assert.equal(calls.at(-1)[1].timeout, 10000)
  })
}

test("supervisor ownership and selected test must agree with scenario", () => {
  for (const [scenario, change] of [
    [scenarios[0], { test: "another-test" }],
    [scenarios[0], { supervisor_pid: 7 }],
    [scenarios[1], { supervisor_pid: 1 }],
    [scenarios[1], { supervisor_ppid: 7 }],
    [scenarios[0], { test_exit: 0 }],
  ])
    assert.throws(
      () => validate({ ...report(scenario), ...change }, scenario),
      /Harness failure/,
    )
})

test("a failed removal prevents acceptance", async () => {
  await assert.rejects(
    runScenario(options, scenarios[0], async (args) => {
      if (args[0] === "rm") throw new Error("removal failed")
      return args[0] === "start" ? JSON.stringify(report(scenarios[0])) : ""
    }),
    /removal failed/,
  )
})

test("rejected evidence is saved before removal", async () => {
  const events = []
  const rejected = { ...report(scenarios[0]), new_orphans: [] }
  await assert.rejects(
    runScenario(
      { ...options, recordEvidence: async (entry) => events.push(entry) },
      scenarios[0],
      async (args) => {
        if (args[0] === "rm") {
          assert.equal(events.at(-1).accepted, false)
          assert.match(events.at(-1).failure, /zombie/)
          assert.deepEqual(events.at(-1).report, rejected)
          assert.equal(events.at(-1).output, JSON.stringify(rejected))
        }
        return args[0] === "start" ? JSON.stringify(rejected) : ""
      },
    ),
    /zombie/,
  )
})

test("malformed output and Docker failure retain diagnostics before removal", async () => {
  for (const failure of [
    "not json",
    new DockerFailure("start failed", "partial stdout", "bounded stderr"),
  ]) {
    const events = []
    await assert.rejects(
      runScenario(
        { ...options, recordEvidence: async (entry) => events.push(entry) },
        scenarios[0],
        async (args) => {
          if (args[0] === "start") {
            if (failure instanceof Error) throw failure
            return failure
          }
          if (args[0] === "rm")
            assert.equal(
              events.at(-1).output,
              typeof failure === "string" ? failure : failure.stdout,
            )
          return ""
        },
      ),
    )
    if (failure instanceof DockerFailure)
      assert.equal(events.at(-1).stderr, failure.stderr)
  }
})

test("evidence failure still removes the container", async () => {
  let removed = false
  await assert.rejects(
    runScenario(
      {
        ...options,
        recordEvidence: async () => {
          throw new Error("evidence write failed")
        },
      },
      scenarios[0],
      async (args) => {
        if (args[0] === "rm") removed = true
        return args[0] === "start" ? JSON.stringify(report(scenarios[0])) : ""
      },
    ),
    /evidence write failed/,
  )
  assert.equal(removed, true)
})

test("truncated output cannot be accepted", () => {
  assert.throws(
    () => validate({ ...report(scenarios[0]), output_truncated: true }, scenarios[0]),
    /truncated/,
  )
})

function launcher(onLaunch, onChild) {
  return (command, args, options) => {
    onLaunch(command, args, options)
    const child = new EventEmitter()
    child.stdout = new EventEmitter()
    child.stderr = new EventEmitter()
    child.kill = () => {
      child.emit("close", null)
      return true
    }
    queueMicrotask(() => onChild(child))
    return child
  }
}

test("Docker uses the managed socket, bounded deadline and supplied cancellation", async () => {
  const signal = new AbortController().signal
  const environment = Object.fromEntries(
    [
      "DOCKER_HOST",
      "DOCKER_CONTEXT",
      "DOCKER_TLS",
      "DOCKER_TLS_VERIFY",
      "DOCKER_CERT_PATH",
    ].map((key) => [key, "inherited-selector"]),
  )
  environment.DOCKER_CONFIG = "/preserved/config"
  environment.HTTPS_PROXY = "configured-proxy"
  await docker(["info"], {
    signal,
    environment,
    launch: launcher(
      (command, args, options) => {
        assert.equal(command, "docker")
        assert.deepEqual(args, ["--host=unix:///var/run/docker.sock", "info"])
        assert.equal(options.signal, signal)
        assert.equal(options.timeout, 30000)
        assert.equal(options.killSignal, "SIGKILL")
        for (const key of [
          "DOCKER_HOST",
          "DOCKER_CONTEXT",
          "DOCKER_TLS",
          "DOCKER_TLS_VERIFY",
          "DOCKER_CERT_PATH",
        ])
          assert.equal(Object.hasOwn(options.env, key), false)
        assert.equal(options.env.DOCKER_CONFIG, environment.DOCKER_CONFIG)
        assert.equal(options.env.HTTPS_PROXY, environment.HTTPS_PROXY)
      },
      (child) => child.emit("close", 0),
    ),
  })
})

test("Docker output overflow kills the client and keeps bounded failure diagnostics", async () => {
  let killed = false
  await assert.rejects(
    docker(["start"], {
      launch: launcher(
        () => {},
        (child) => {
          child.kill = () => {
            killed = true
            queueMicrotask(() => child.emit("close", null))
            return true
          }
          child.stdout.emit("data", Buffer.alloc(129 * 1024, 97))
          child.emit("close", 0)
        },
      ),
    }),
    (error) => {
      assert.ok(error instanceof DockerFailure)
      assert.match(error.message, /output exceeded/)
      assert.equal(Buffer.byteLength(error.stdout), 128 * 1024)
      return true
    },
  )
  assert.equal(killed, true)
})

test("libtest output must name the selected test and account for one active test", () => {
  const positive = scenarios[1]
  const output = report(positive).output
  for (const changed of [
    output.replace(positive.test, "another-test"),
    output.replace("1 passed; 0 failed", "1 passed; 1 failed"),
    output.replace("0 ignored", "1 ignored"),
    output.replace("test result: ok.", "no summary"),
  ])
    assert.throws(
      () => validate({ ...report(positive), output: changed }, positive),
      /Harness failure/,
    )
})

test("libtest counts, status and process exit cannot contradict each other", () => {
  for (const [scenario, output] of [
    [
      scenarios[1],
      report(scenarios[1]).output.replace("running 1 test", "running 2 tests"),
    ],
    [
      scenarios[1],
      report(scenarios[1]).output.replace("test result: ok.", "test result: FAILED."),
    ],
    [
      scenarios[1],
      report(scenarios[1]).output.replace("1 passed; 0 failed", "0 passed; 1 failed"),
    ],
    [
      scenarios[0],
      report(scenarios[0]).output.replace("test result: FAILED.", "test result: ok."),
    ],
    [
      scenarios[0],
      report(scenarios[0]).output.replace("0 passed; 1 failed", "1 passed; 0 failed"),
    ],
  ])
    assert.throws(
      () => validate({ ...report(scenario), output }, scenario),
      /Harness failure/,
    )
})

test("the complete harness pins the inspected image and records four removed acceptances", async () => {
  const root = await mkdtemp(join(tmpdir(), "nessa-cleanup-orchestration-"))
  try {
    const binary = join(root, "explicit-test-binary")
    const manifest = join(root, "compiled-sdk")
    const fixture = join(manifest, "tests/infrastructure/acp/contracts/fixtures")
    const evidence = join(root, "evidence")
    await writeFile(binary, "explicit library test")
    await mkdir(fixture, { recursive: true })
    await writeFile(join(fixture, "claude_acp_test_handler.py"), "fixture")
    let active
    const created = [],
      removed = []
    await checkContainers([binary, manifest, "mutable-tag", evidence], async (args) => {
      if (args[0] === "image") return JSON.stringify([{ Id: "sha256:immutable-image" }])
      if (args[0] === "create") {
        created.push(args)
        active = scenarios.find(
          (scenario) =>
            scenario.test === args.at(-1) && scenario.init === args.includes("--init"),
        )
      }
      if (args[0] === "rm") removed.push(args.at(-1))
      return args[0] === "start" ? JSON.stringify(report(active)) : ""
    })
    const saved = JSON.parse(await readFile(join(evidence, "acceptance.json"), "utf8"))
    assert.equal(saved.image, "sha256:immutable-image")
    assert.equal(saved.imageReference, "mutable-tag")
    assert.match(saved.binarySha256, /^[a-f0-9]{64}$/)
    assert.equal(saved.reports.length, 4)
    assert.ok(
      saved.reports.every((entry) => entry.accepted && removed.includes(entry.container)),
    )
    assert.ok(created.every((args) => args.at(-2) === saved.image))
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

test("missing or nonnumeric negative exit evidence cannot pass", () => {
  for (const test_exit of [undefined, null, "101"])
    assert.throws(
      () => validate({ ...report(scenarios[0]), test_exit }, scenarios[0]),
      /integer test exit/,
    )
})

test("repeated interrupts keep cancellation installed until removal finishes", async () => {
  const root = await mkdtemp(join(tmpdir(), "nessa-cleanup-interrupt-"))
  const initialListeners = process.listenerCount("SIGINT")
  try {
    const binary = join(root, "binary")
    const fixture = join(root, "tests/infrastructure/acp/contracts/fixtures")
    await writeFile(binary, "library test")
    await mkdir(fixture, { recursive: true })
    await writeFile(join(fixture, "claude_acp_test_handler.py"), "fixture")
    let removed = false
    await assert.rejects(
      checkContainers(
        [binary, root, "image", join(root, "evidence")],
        async (args, operation) => {
          if (args[0] === "image") return JSON.stringify([{ Id: "sha256:immutable" }])
          if (args[0] === "start") {
            process.emit("SIGINT")
            process.emit("SIGINT")
            assert.equal(operation.signal.aborted, true)
            assert.equal(process.listenerCount("SIGINT"), initialListeners + 1)
            throw new Error("interrupted test")
          }
          if (args[0] === "rm") {
            process.emit("SIGINT")
            assert.equal(process.listenerCount("SIGINT"), initialListeners + 1)
            assert.equal(operation.signal, undefined)
            removed = true
          }
          return ""
        },
      ),
      /interrupted test/,
    )
    assert.equal(removed, true)
    assert.equal(process.listenerCount("SIGINT"), initialListeners)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

test("incomplete proof fields cannot produce a positive or negative acceptance", () => {
  for (const scenario of [scenarios[0], scenarios[1]]) {
    for (const key of [
      "supervisor_pid",
      "supervisor_ppid",
      "test_pid",
      "timed_out",
      "output_truncated",
      "initial_processes",
      "new_orphans",
      "retained_directories",
      "pid1_executable",
      "packages",
      "test",
      "output",
    ])
      assert.throws(
        () => validate({ ...report(scenario), [key]: undefined }, scenario),
        /Harness failure/,
        `${scenario.init}: ${key}`,
      )
    for (const change of [
      { supervisor_pid: "7" },
      { supervisor_pid: null },
      { supervisor_pid: 0 },
      { supervisor_ppid: "1" },
      { test_pid: null },
      { test_pid: "8" },
      { test_pid: 0 },
      { timed_out: null },
      { output_truncated: 0 },
      { output: null },
      { pid1_executable: "" },
      { initial_processes: {} },
      { new_orphans: {} },
      { retained_directories: {} },
      { packages: {} },
    ])
      assert.throws(
        () => validate({ ...report(scenario), ...change }, scenario),
        /Harness failure/,
      )
  }
  for (const malformed of [null, [], "report"])
    assert.throws(() => validate(malformed, scenarios[1]), /Harness failure/)
})

test("zombie, directory and package evidence must identify concrete captured resources", () => {
  const negative = report(scenarios[0])
  for (const new_orphans of [
    [{ ppid: 1, state: "Z" }],
    [{ ppid: 1, pgid: 9, state: "Z" }],
    [{ pid: 10, ppid: 1, state: "Z" }],
    [{ pid: 10, ppid: 1, pgid: "9", state: "Z" }],
    [{ pid: 10, ppid: 1, pgid: 9, state: undefined }],
    [null],
  ])
    assert.throws(
      () => validate({ ...negative, new_orphans }, scenarios[0]),
      /Harness failure/,
    )
  for (const retained_directories of [
    [{}],
    [null],
    ["elsewhere"],
    ["/tmp/nessa-agent-nested/path"],
  ])
    assert.throws(
      () => validate({ ...negative, retained_directories }, scenarios[0]),
      /Harness failure/,
    )
  assert.throws(
    () =>
      validate({ ...negative, packages: Object.create(negative.packages) }, scenarios[0]),
    /Harness failure/,
  )
  for (const name of ["python3-minimal", "libgcc-s1", "ca-certificates"])
    for (const value of [undefined, "", 3])
      assert.throws(
        () =>
          validate(
            { ...negative, packages: { ...negative.packages, [name]: value } },
            scenarios[0],
          ),
        /Harness failure/,
      )
})

test("captured process identities cannot be reused or contradict the supervisor", () => {
  const positive = report(scenarios[1])
  for (const change of [
    { test_pid: positive.supervisor_pid },
    { initial_processes: [] },
    { initial_processes: positive.initial_processes.slice(1) },
    { initial_processes: [...positive.initial_processes, positive.initial_processes[1]] },
    { initial_processes: positive.initial_processes.map((p) => ({ ...p, ppid: 3 })) },
    { new_orphans: [{ pid: positive.test_pid, ppid: 1, pgid: 9, state: "S" }] },
    { new_orphans: [{ pid: positive.supervisor_pid, ppid: 1, pgid: 9, state: "S" }] },
    {
      new_orphans: [
        { pid: 10, ppid: 1, pgid: 9, state: "S" },
        { pid: 10, ppid: 1, pgid: 9, state: "S" },
      ],
    },
  ])
    assert.throws(
      () => validate({ ...positive, ...change }, scenarios[1]),
      /Harness failure/,
    )
})

test("image inspection receives interrupt cancellation before any container is created", async () => {
  const root = await mkdtemp(join(tmpdir(), "nessa-cleanup-inspection-"))
  const initialListeners = process.listenerCount("SIGINT")
  try {
    const binary = join(root, "binary")
    const fixture = join(root, "tests/infrastructure/acp/contracts/fixtures")
    await writeFile(binary, "library test")
    await mkdir(fixture, { recursive: true })
    await writeFile(join(fixture, "claude_acp_test_handler.py"), "fixture")
    const calls = []
    await assert.rejects(
      checkContainers(
        [binary, root, "image", join(root, "evidence")],
        async (args, operation) => {
          calls.push(args[0])
          assert.equal(args[0], "image")
          assert.ok(operation.signal instanceof AbortSignal)
          process.emit("SIGINT")
          assert.equal(operation.signal.aborted, true)
          throw new Error("inspection interrupted")
        },
      ),
      /inspection interrupted/,
    )
    assert.deepEqual(calls, ["image"])
    assert.equal(process.listenerCount("SIGINT"), initialListeners)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

test("all captured process entries require typed fields even when they are not zombies", () => {
  const positive = report(scenarios[1])
  for (const process of [
    { pid: 20, ppid: "1", pgid: 9, state: "S" },
    { pid: 20, ppid: -1, pgid: 9, state: "S" },
    { pid: 20, ppid: 1, pgid: 9, state: undefined },
    { pid: 20, ppid: 1, pgid: 9, state: "SS" },
  ])
    assert.throws(
      () =>
        validate(
          { ...positive, initial_processes: [...positive.initial_processes, process] },
          scenarios[1],
        ),
      /Harness failure/,
    )
  assert.throws(
    () =>
      validate(
        { ...positive, new_orphans: [{ pid: 20, ppid: 1, pgid: 9, state: undefined }] },
        scenarios[1],
      ),
    /Harness failure/,
  )
})

test("negative failure proof names the selected panic and its actual cause", () => {
  for (const scenario of [scenarios[0], scenarios[2]]) {
    const negative = report(scenario)
    const output = negative.output
    for (const changed of [
      output.replace("value: CleanupUncertain", "value: DeadlineExceeded") +
        "\nincidental CleanupUncertain",
      output.replace(`thread '${scenario.test}'`, "thread 'another-test'"),
      output.replace(
        `thread '${scenario.test}'`,
        `thread '${scenario.test.slice(0, -1)}x'`,
      ),
      output.replace(`thread '${scenario.test}'`, `thread '${scenario.test}-other'`),
      output
        .replace(
          "called `Result::unwrap()`",
          "debug CleanupUncertain\ncalled `Result::unwrap()`",
        )
        .replace("value: CleanupUncertain", "value: DeadlineExceeded"),
    ])
      assert.throws(
        () => validate({ ...negative, output: changed }, scenario),
        /CleanupUncertain/,
      )
    for (const supported of [
      output.replace(" (8) panicked", " panicked"),
      output.replaceAll("\n", "\r\n"),
    ])
      assert.equal(
        validate({ ...negative, output: supported }, scenario).test,
        scenario.test,
      )
  }
})

test("the acceptance publisher never writes accepted evidence when already interrupted", async () => {
  const controller = new AbortController()
  controller.abort()
  const entries = []
  await assert.rejects(
    publishAcceptance(
      { container: "removed", accepted: false },
      controller.signal,
      async (entry) => entries.push(entry),
    ),
    /interrupted/,
  )
  assert.ok(entries.length > 0 && entries.every((entry) => !entry.accepted))
})

test("an acceptance write failure retains unaccepted diagnostic evidence", async () => {
  const entries = []
  await assert.rejects(
    publishAcceptance(
      { container: "removed", accepted: false },
      new AbortController().signal,
      async (entry) => {
        if (entry.accepted) throw new Error("acceptance write failed")
        entries.push(entry)
      },
    ),
    /acceptance write failed/,
  )
  assert.equal(entries.at(-1).accepted, false)
  assert.equal(entries.at(-1).failure, "acceptance write failed")
})

for (const interruptAt of ["last-removal", "final-write"]) {
  test(`interrupt during ${interruptAt} rejects four-case acceptance after independent removal`, async () => {
    const root = await mkdtemp(join(tmpdir(), "nessa-cleanup-publication-"))
    try {
      const binary = join(root, "binary")
      const fixture = join(root, "tests/infrastructure/acp/contracts/fixtures")
      const evidence = join(root, "evidence")
      await writeFile(binary, "library test")
      await mkdir(fixture, { recursive: true })
      await writeFile(join(fixture, "claude_acp_test_handler.py"), "fixture")
      let active
      const removed = []
      const run = async (args, operation) => {
        if (args[0] === "image") return JSON.stringify([{ Id: "sha256:immutable" }])
        if (args[0] === "create")
          active = scenarios.find(
            (scenario) =>
              scenario.test === args.at(-1) && scenario.init === args.includes("--init"),
          )
        if (args[0] === "rm") {
          assert.equal(operation.signal, undefined)
          removed.push(args.at(-1))
          if (interruptAt === "last-removal" && removed.length === 4)
            process.emit("SIGINT")
        }
        return args[0] === "start" ? JSON.stringify(report(active)) : ""
      }
      const persist = async (path, contents) => {
        await writeFile(path, contents)
        const saved = JSON.parse(contents)
        if (
          interruptAt === "final-write" &&
          saved.reports.length === 4 &&
          saved.reports.at(-1).accepted
        )
          process.emit("SIGINT")
      }
      await assert.rejects(
        checkContainers([binary, root, "image", evidence], run, persist),
        /interrupted/,
      )
      const saved = JSON.parse(await readFile(join(evidence, "acceptance.json"), "utf8"))
      assert.equal(removed.length, 4)
      assert.equal(saved.reports.length, 4)
      assert.equal(saved.reports.at(-1).accepted, false)
      assert.match(saved.reports.at(-1).failure, /interrupted/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
}

test("both Docker streams preserve Unicode split across chunks", async () => {
  for (const stream of ["stdout", "stderr"]) {
    const encoded = Buffer.from("before€😀after")
    const launch = launcher(
      () => {},
      (child) => {
        for (let offset = 0; offset < encoded.length; offset++)
          child[stream].emit("data", encoded.subarray(offset, offset + 1))
        child.emit("close", stream === "stdout" ? 0 : 1)
      },
    )
    if (stream === "stdout")
      assert.equal(await docker(["start"], { launch }), "before€😀after")
    else
      await assert.rejects(docker(["start"], { launch }), (error) => {
        assert.equal(error.stderr, "before€😀after")
        assert.match(error.message, /exited 1/)
        return true
      })
  }
})

test("multibyte diagnostics exactly at the byte budget fit on both streams", async () => {
  const expected = "a".repeat(128 * 1024 - 3) + "€"
  for (const stream of ["stdout", "stderr"]) {
    const launch = launcher(
      () => {},
      (child) => {
        child[stream].emit("data", Buffer.from(expected))
        child.emit("close", stream === "stdout" ? 0 : 1)
      },
    )
    if (stream === "stdout") assert.equal(await docker(["start"], { launch }), expected)
    else
      await assert.rejects(docker(["start"], { launch }), (error) => {
        assert.equal(error.stderr, expected)
        assert.match(error.message, /exited 1/)
        return true
      })
  }
})

test("a multibyte character crossing the byte budget rejects and retains a valid prefix", async () => {
  const prefix = "a".repeat(128 * 1024 - 1)
  for (const stream of ["stdout", "stderr"]) {
    let killed = false
    await assert.rejects(
      docker(["start"], {
        launch: launcher(
          () => {},
          (child) => {
            child.kill = () => {
              killed = true
              queueMicrotask(() => child.emit("close", 0))
              return true
            }
            child[stream].emit("data", Buffer.from(prefix + "€"))
            child.emit("close", 0)
          },
        ),
      }),
      (error) => {
        assert.match(error.message, /output exceeded/)
        assert.equal(error[stream], prefix)
        assert.ok(Buffer.byteLength(error[stream]) <= 128 * 1024)
        return true
      },
    )
    assert.equal(killed, true)
  }
})

test("malformed UTF-8 expansion and unfinished sequences respect diagnostic byte budgets", async () => {
  for (const stream of ["stdout", "stderr"]) {
    for (const suffix of [Buffer.from([0xff]), Buffer.from([0xe2, 0x82])]) {
      const prefix = "a".repeat(128 * 1024 - 2)
      await assert.rejects(
        docker(["start"], {
          launch: launcher(
            () => {},
            (child) => {
              child.kill = () => {
                queueMicrotask(() => child.emit("close", 0))
                return true
              }
              child[stream].emit("data", Buffer.from(prefix))
              child[stream].emit("data", suffix)
              child.emit("close", 0)
            },
          ),
        }),
        (error) => {
          assert.match(error.message, /output exceeded/)
          assert.equal(error[stream], prefix)
          assert.ok(Buffer.byteLength(error[stream]) <= 128 * 1024)
          return true
        },
      )
    }
  }
})

test("malformed bytes that fit are retained as correctly budgeted replacement characters", async () => {
  const prefix = "a".repeat(128 * 1024 - 3)
  const expected = prefix + "\ufffd"
  const actual = await docker(["start"], {
    launch: launcher(
      () => {},
      (child) => {
        child.stdout.emit("data", Buffer.from(prefix))
        child.stdout.emit("data", Buffer.from([0xff]))
        child.emit("close", 0)
      },
    ),
  })
  assert.equal(actual, expected)
  assert.equal(Buffer.byteLength(actual), 128 * 1024)
})
