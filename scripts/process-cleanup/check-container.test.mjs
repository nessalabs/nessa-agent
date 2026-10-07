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
  scenarios,
  validate,
} from "./check-container.mjs"

function report(scenario) {
  return {
    test: scenario.test,
    timed_out: false,
    supervisor_pid: scenario.init ? 7 : 1,
    supervisor_ppid: scenario.init ? 1 : 0,
    test_exit: scenario.init ? 0 : 101,
    output: scenario.init
      ? `running 1 test\ntest ${scenario.test} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;`
      : `running 1 test\ntest ${scenario.test} ... FAILED\nCleanupUncertain\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;`,
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
