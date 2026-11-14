import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { fileURLToPath, URL } from "node:url"
import { test } from "node:test"
import { readWorkflowSource } from "../workflow-source.mjs"
import {
  desktopMeasurementPlan,
  DesktopMeasurementScopeError,
} from "./measurement-plan.mjs"

const modulePath = fileURLToPath(new URL("./measurement-plan.mjs", import.meta.url))
const workflow = readWorkflowSource(
  new URL("../../.github/workflows/conversation-stress-load.yml", import.meta.url),
)
const expected = {
  full: {
    scope: "full",
    check: "",
    buildGateway: true,
    artifactPrefix: "desktop-browser-qualification",
  },
  "frame-budget": {
    scope: "frame-budget",
    check: "perf-budget",
    buildGateway: false,
    artifactPrefix: "desktop-frame-budget",
  },
  "mcp-apps": {
    scope: "mcp-apps",
    check: "mcp-apps",
    buildGateway: false,
    artifactPrefix: "desktop-mcp-apps",
  },
}

for (const scope of Object.keys(expected)) {
  test(`${scope} publishes one immutable workload, build and evidence identity`, () => {
    const plan = desktopMeasurementPlan(scope)
    assert.deepEqual(plan, expected[scope])
    assert.equal(Object.isFrozen(plan), true)
    assert.throws(() => {
      plan.check = "smoke"
    }, TypeError)
    assert.deepEqual(desktopMeasurementPlan(scope), expected[scope])
    const command = spawnSync(process.execPath, [modulePath, scope], {
      encoding: "utf8",
      timeout: 5000,
    })
    assert.equal(command.status, 0, command.stderr)
    assert.equal(command.stderr, "")
    assert.deepEqual(
      Object.fromEntries(
        command.stdout
          .trim()
          .split("\n")
          .map((line) => {
            const at = line.indexOf("=")
            return [line.slice(0, at), line.slice(at + 1)]
          }),
      ),
      {
        scope,
        check: plan.check,
        "build-gateway": String(plan.buildGateway),
        "artifact-prefix": plan.artifactPrefix,
      },
    )
  })
}

test("unknown, inherited and non-string scopes fail before publishing a plan", () => {
  const hostile = {
    toString() {
      throw new Error("must not coerce an unknown scope")
    },
  }
  for (const value of [
    "",
    "all",
    "constructor",
    "toString",
    "__proto__",
    null,
    undefined,
    false,
    0,
    hostile,
  ])
    assert.throws(() => desktopMeasurementPlan(value), DesktopMeasurementScopeError)
  for (const args of [[], ["unknown"], ["constructor"], ["full", "extra"]]) {
    const command = spawnSync(process.execPath, [modulePath, ...args], {
      encoding: "utf8",
      timeout: 5000,
    })
    assert.equal(command.status, 2)
    assert.equal(command.stdout, "")
    assert.match(command.stderr, /Expected a desktop measurement scope/)
  }
})

function step(name) {
  const start = workflow.indexOf(`      - name: ${name}\n`)
  assert.ok(start >= 0, name)
  const end = workflow.indexOf("\n      - ", start + 1)
  return workflow.slice(start, end < 0 ? undefined : end)
}
const measurement = step("Measure the selected desktop checks on Mac")
const body = measurement.match(/ {8}run: \|\n([\s\S]*?) {8}timeout-minutes:/)?.[1]
assert.ok(body)
const shell = body
  .split("\n")
  .map((line) => (line.startsWith("          ") ? line.slice(10) : line))
  .join("\n")

test("the workflow consumes one validated plan before deciding builds, selector and artifact", () => {
  assert.match(
    workflow,
    /desktop-scope:\n\s+description: [^\n]+\n\s+type: choice\n\s+options:\n\s+- full\n\s+- frame-budget\n\s+- mcp-apps\n\s+default: full/,
  )
  const planner = step("Plan the desktop measurement")
  assert.match(planner, /if: \$\{\{ inputs\['desktop-performance'\] \}\}/)
  assert.match(planner, /DESKTOP_SCOPE: \$\{\{ inputs\['desktop-scope'\] \}\}/)
  assert.match(
    planner,
    /run: node scripts\/desktop\/measurement-plan\.mjs "\$DESKTOP_SCOPE" >> "\$GITHUB_OUTPUT"/,
  )
  assert.ok(
    workflow.indexOf("name: Plan the desktop measurement") <
      workflow.indexOf("run: cargo build"),
  )
  assert.match(
    workflow,
    /run: cargo build -p nessa-server -p nessa-mcp\n\s+if: \$\{\{ !inputs\['desktop-performance'\] \|\| steps\.desktop-plan\.outputs\.build-gateway == 'true' \}\}/,
  )
  assert.match(
    measurement,
    /DESKTOP_CHECK: \$\{\{ steps\.desktop-plan\.outputs\.check \}\}/,
  )
  assert.match(measurement, /timeout-minutes: 135/)
  const artifact = step("Keep the evidence")
  assert.match(
    artifact,
    /steps\.desktop-plan\.outputs\.artifact-prefix \|\| 'desktop-measurement-incomplete'/,
  )
  assert.match(artifact, /\|\| 'conversation-stress-load'/)
  assert.match(artifact, /retention-days: 14/)
})

function execute(scope, status = 0, check = desktopMeasurementPlan(scope).check) {
  const root = mkdtempSync(join(tmpdir(), "nessa measurement-"))
  try {
    const directory = join(root, "verification/desktop/scripts")
    mkdirSync(directory, { recursive: true })
    writeFileSync(
      join(directory, "run-all.mjs"),
      `import{writeFileSync}from'node:fs';writeFileSync(process.env.ARGV_CAPTURE,JSON.stringify(process.argv.slice(2)));process.exit(Number(process.env.CHILD_STATUS));\n`,
    )
    const output = join(root, "args.json"),
      evidence = join(root, "evidence with spaces")
    const result = spawnSync("bash", ["-e", "-o", "pipefail", "-c", shell], {
      cwd: root,
      env: {
        ...process.env,
        DESKTOP_CHECK: check,
        STRESS_EVIDENCE: evidence,
        ARGV_CAPTURE: output,
        CHILD_STATUS: String(status),
      },
      encoding: "utf8",
      timeout: 5000,
    })
    assert.equal(result.error, undefined)
    assert.equal(result.status, status, result.stderr)
    const args = JSON.parse(readFileSync(output, "utf8"))
    return {
      args,
      expectedBase: [
        "--headed",
        "--layout",
        "columns,sidebar",
        "--channel",
        "bundled",
        "--runs",
        "5",
        "--out",
        join(evidence, "desktop-performance.json"),
        "--reports",
        join(evidence, "desktop-checks"),
      ],
    }
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

// The workflow runs only on standard Linux/Mac runners, whose shell is Bash.
// Plan and CLI tests above remain portable to bare Node on Windows.
for (const scope of Object.keys(expected))
  test(
    `${scope} actual workflow script preserves arguments and child outcomes`,
    { skip: process.platform === "win32" },
    () => {
      for (const status of [0, 1, 2]) {
        const { args, expectedBase } = execute(scope, status)
        assert.deepEqual(args, [
          ...expectedBase,
          ...(scope === "full" ? [] : ["--only", expected[scope].check]),
        ])
      }
    },
  )

test(
  "the published check selector is one quoted argument, not shell syntax",
  { skip: process.platform === "win32" },
  () => {
    const check = "perf-budget with spaces; $(false)"
    const { args, expectedBase } = execute("frame-budget", 0, check)
    assert.deepEqual(args, [...expectedBase, "--only", check])
  },
)
