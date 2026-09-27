import assert from "node:assert/strict"
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"
import { spawnSync } from "node:child_process"
import test from "node:test"

test(
  "the previous dev server is removed before Tauri may observe its port",
  { skip: process.platform === "win32" },
  (context) => {
    const directory = mkdtempSync(join(tmpdir(), "nessa-launch-order-"))
    context.after(() => rmSync(directory, { recursive: true, force: true }))
    const events = join(directory, "events")
    for (const command of ["node", "pnpm"]) {
      const executable = join(directory, command)
      writeFileSync(
        executable,
        `#!/bin/sh\nprintf '%s\\n' '${command} '$* >> "$NESSA_TEST_EVENTS"\n`,
      )
      chmodSync(executable, 0o755)
    }

    const result = spawnSync("bash", [resolve("scripts/run-dev-app.sh")], {
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: `${directory}:${process.env.PATH}`,
        NESSA_TEST_EVENTS: events,
      },
    })
    assert.equal(result.status, 0, result.stderr)
    const calls = readFileSync(events, "utf8").trim().split("\n")
    assert.match(calls[0], /^node .*free-dev-port\.mjs$/)
    assert.equal(calls[1], "pnpm app")
  },
)

test("desktop dev forwards literal Tauri arguments through the Node pnpm entry", (context) => {
  const directory = mkdtempSync(join(tmpdir(), "nessa-desktop-dev-"))
  context.after(() => rmSync(directory, { recursive: true, force: true }))
  const events = join(directory, "events")
  const pnpm = join(directory, "pnpm.mjs")
  writeFileSync(
    pnpm,
    `import { writeFileSync } from "node:fs"
writeFileSync(process.env.NESSA_TEST_EVENTS, JSON.stringify({
  args: process.argv.slice(2),
  hostStage: process.env.NESSA_STAGE,
  uiStage: process.env.VITE_NESSA_STAGE,
}))
`,
  )

  const result = spawnSync(
    process.execPath,
    [
      resolve("scripts/desktop/dev.mjs"),
      "--help",
      "--config",
      "alternate config.json",
      "--release",
      "$(literal)",
      "one&two",
    ],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        npm_execpath: pnpm,
        NESSA_TEST_EVENTS: events,
      },
    },
  )

  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(JSON.parse(readFileSync(events, "utf8")), {
    args: [
      "exec",
      "tauri",
      "dev",
      "--help",
      "--config",
      "alternate config.json",
      "--release",
      "$(literal)",
      "one&two",
    ],
    hostStage: "dev",
    uiStage: "dev",
  })
})

test("desktop dev explains how to supply its Node pnpm entry", () => {
  const environment = { ...process.env }
  delete environment.npm_execpath
  const result = spawnSync(process.execPath, [resolve("scripts/desktop/dev.mjs")], {
    encoding: "utf8",
    env: environment,
  })

  assert.equal(result.status, 1)
  assert.match(result.stderr, /through `pnpm app`/)
})

test(
  "start waits for successful preflight before it starts the gateway",
  { skip: process.platform === "win32" },
  (context) => {
    const directory = mkdtempSync(join(tmpdir(), "nessa-start-order-"))
    context.after(() => rmSync(directory, { recursive: true, force: true }))
    const events = join(directory, "events")
    const realJust = spawnSync("sh", ["-c", "command -v just"], {
      encoding: "utf8",
    }).stdout.trim()
    assert.ok(realJust, "just is required to exercise its public start recipe")
    const commands = {
      node: `#!/bin/sh
printf 'node %s\\n' "$1" >> "$NESSA_TEST_EVENTS"
case "$1" in
  *gateway-port.mjs) printf '7999' ;;
  *preflight.mjs) exit "\${NESSA_PREFLIGHT_STATUS:-0}" ;;
esac
`,
      pnpm: `#!/bin/sh
for _ in $(seq 1 500); do
  [ -f "$NESSA_TEST_EVENTS.first-health-check" ] && break
  sleep 0.01
done
[ -f "$NESSA_TEST_EVENTS.first-health-check" ] || exit 1
printf 'pnpm %s\\n' "$*" >> "$NESSA_TEST_EVENTS"
touch "$NESSA_TEST_EVENTS.gateway-started"
while :; do sleep 1; done
`,
      curl: `#!/bin/sh
if [ -f "$NESSA_TEST_EVENTS.gateway-started" ]; then
  printf 'curl ready %s\\n' "$*" >> "$NESSA_TEST_EVENTS"
  exit 0
fi
printf 'curl not-ready %s\\n' "$*" >> "$NESSA_TEST_EVENTS"
touch "$NESSA_TEST_EVENTS.first-health-check"
exit 1
`,
      just: `#!/bin/sh
printf 'just %s\\n' "$*" >> "$NESSA_TEST_EVENTS"
`,
    }
    for (const [command, source] of Object.entries(commands)) {
      const executable = join(directory, command)
      writeFileSync(executable, source)
      chmodSync(executable, 0o755)
    }
    const run = (status) => {
      writeFileSync(events, "")
      rmSync(`${events}.first-health-check`, { force: true })
      rmSync(`${events}.gateway-started`, { force: true })
      return spawnSync(realJust, ["start", "dev"], {
        encoding: "utf8",
        env: {
          ...process.env,
          PATH: `${directory}:${process.env.PATH}`,
          NESSA_PREFLIGHT_STATUS: String(status),
          NESSA_TEST_EVENTS: events,
        },
      })
    }

    const failed = run(1)
    assert.equal(failed.status, 1)
    const failedCalls = readFileSync(events, "utf8")
    assert.match(failedCalls, /preflight\.mjs/)
    assert.doesNotMatch(failedCalls, /pnpm server:run/)
    assert.doesNotMatch(failedCalls, /curl /)

    const succeeded = run(0)
    assert.equal(succeeded.status, 0, succeeded.stderr)
    const calls = readFileSync(events, "utf8").trim().split("\n")
    const preflight = calls.findIndex((call) => call.includes("preflight.mjs"))
    const firstHealthCheck = calls.findIndex((call) => call.startsWith("curl "))
    const gatewayStarted = calls.indexOf("pnpm server:run")
    const gatewayReady = calls.findIndex((call) => call.startsWith("curl ready "))
    const uiStarted = calls.indexOf("just dev")
    assert.ok(preflight >= 0 && preflight < firstHealthCheck)
    assert.ok(
      calls
        .slice(firstHealthCheck, gatewayStarted)
        .some((call) => call.startsWith("curl not-ready ")),
      "the first health check did not exercise the startup gate",
    )
    assert.ok(gatewayStarted >= 0 && gatewayStarted < gatewayReady)
    assert.ok(gatewayReady >= 0 && gatewayReady < uiStarted)
  },
)

test("preflight performs the gateway build synchronously", () => {
  const recipe = readFileSync("justfile", "utf8")
  const preflight = recipe.indexOf('node scripts/preflight.mjs "${stage}"')
  const server = recipe.indexOf("pnpm server:run &", preflight)
  const health = recipe.indexOf("curl -sf --connect-timeout 0.3", server)
  assert.ok(preflight >= 0 && preflight < server && server < health)

  const preparation = readFileSync("scripts/preflight.mjs", "utf8")
  assert.match(preparation, /execFileSync\("cargo", \["build", "-p", "nessa-server"\]/)
})

test(
  "release passes its named stage to the desktop build",
  { skip: process.platform === "win32" },
  (context) => {
    const directory = mkdtempSync(join(tmpdir(), "nessa-release-stage-"))
    context.after(() => rmSync(directory, { recursive: true, force: true }))
    const events = join(directory, "events")
    const node = join(directory, "node")
    writeFileSync(node, `#!/bin/sh\nprintf '%s\\n' "$*" >> "$NESSA_TEST_EVENTS"\n`)
    chmodSync(node, 0o755)
    const realJust = spawnSync("sh", ["-c", "command -v just"], {
      encoding: "utf8",
    }).stdout.trim()
    assert.ok(realJust, "just is required to exercise its public release recipe")
    const run = (...args) =>
      spawnSync(realJust, ["release", ...args], {
        encoding: "utf8",
        env: {
          ...process.env,
          PATH: `${directory}:${process.env.PATH}`,
          NESSA_TEST_EVENTS: events,
        },
      })

    assert.equal(run().status, 0)
    assert.equal(run("alpha").status, 0)
    assert.equal(run("dev", "fast").status, 0)
    const calls = readFileSync(events, "utf8").trim().split("\n")
    // Linux names no bundles: its build makes exactly the release's .deb and
    // refuses a choice.
    const bundles = process.platform === "linux" ? /^$/ : /^ --bundles \S+$/
    for (const [call, stage] of [
      [calls[0], "prod"],
      [calls[1], "alpha"],
      [calls[2], "dev"],
    ]) {
      const [, rest] = call.match(new RegExp(`build\\.mjs --stage ${stage}(.*)$`)) ?? []
      assert.notEqual(rest, undefined, call)
      assert.match(rest.trimEnd(), bundles, call)
    }
  },
)
