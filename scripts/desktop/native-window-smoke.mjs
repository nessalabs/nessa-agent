#!/usr/bin/env node
/**
 * Linux native-window smoke: embedded frontend, gateway, ACP boundary, and cleanup.
 *
 * WebDriver launches the real debug executable and controls its WebKitGTK page.
 * The synthetic File drop reaches the page's real FileDropZone and upload path;
 * it does not exercise the native OS drag handler, which owns physical file drops.
 */
import assert from "node:assert/strict"
import { randomUUID } from "node:crypto"
import { spawn, spawnSync } from "node:child_process"
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { createServer } from "node:net"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { setTimeout as delay } from "node:timers/promises"
import { fileURLToPath } from "node:url"

import {
  alive,
  recordedPid,
  stopOwnedGroup,
  stopRecordedProcess,
  watchInterruptions,
} from "./native-smoke-processes.mjs"
import { builtExecutables } from "./native-smoke-build.mjs"
import {
  renderNativeSmokeError,
  retainNativeSmokeFailure,
} from "./native-smoke-evidence.mjs"
import {
  observeWebdriverStartup,
  webdriverBudgets,
  webdriverRequest,
} from "./native-smoke-webdriver.mjs"

const root = join(dirname(fileURLToPath(import.meta.url)), "../..")
const elementKey = "element-6066-11e4-a52e-4f735466cecf"
const png =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="

if (process.platform !== "linux") {
  console.log("native-window smoke skipped: Tauri WebDriver supports Linux and Windows")
  process.exit(0)
}

function checked(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: root,
    env: process.env,
    encoding: "utf8",
    stdio: "inherit",
    ...options,
  })
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error(`${command} exited with ${result.status}`)
}

function requireCommand(command) {
  const result = spawnSync("sh", ["-c", `command -v "$1"`, "sh", command], {
    encoding: "utf8",
  })
  if (result.status !== 0)
    throw new Error(`${command} is required for the native-window smoke`)
}

function shellQuote(value) {
  return `'${String(value).replaceAll("'", `'"'"'`)}'`
}

async function freePort() {
  const server = createServer()
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  assert.equal(typeof address, "object")
  const port = address.port
  await new Promise((resolve, reject) =>
    server.close((error) => (error ? reject(error) : resolve())),
  )
  return port
}

async function eventually(
  description,
  action,
  timeout = 30_000,
  signal = interruption.signal,
) {
  const deadline = Date.now() + timeout
  let last
  while (Date.now() < deadline) {
    if (signal?.aborted) throw signal.reason
    try {
      const value = await action()
      if (value) return value
    } catch (error) {
      last = error
    }
    await delay(100)
  }
  throw new Error(
    `${description} did not happen within ${timeout}ms${last ? `: ${last}` : ""}`,
  )
}

function capture(child, label, entries) {
  for (const stream of [child.stdout, child.stderr])
    stream?.on("data", (chunk) => {
      entries.push(`[${label}] ${chunk}`)
      if (entries.length > 300) entries.shift()
    })
}

function embeddedCsp(port) {
  const config = JSON.parse(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"))
  const csp = config.app.security.csp
  const origins = ` ws://127.0.0.1:${port} http://127.0.0.1:${port}`
  const clauses = csp.split(";").map((clause) => clause.trim())
  const connect = clauses.findIndex((clause) => clause.startsWith("connect-src "))
  assert.notEqual(connect, -1, "tauri.conf.json CSP must declare connect-src")
  clauses[connect] += origins
  return clauses.filter(Boolean).join("; ")
}

async function webdriver(
  port,
  method,
  path,
  body,
  { phase, lifecycle = "command", signal = interruption.signal } = {},
) {
  return webdriverRequest({
    port,
    method,
    path,
    body,
    phase: phase ?? `${method} ${path}`,
    lifecycle,
    signal,
  })
}

async function element(port, session, selector) {
  const value = await webdriver(
    port,
    "POST",
    `/session/${session}/element`,
    { using: "css selector", value: selector },
    { phase: `find ${selector}` },
  )
  return value[elementKey] ?? value.ELEMENT
}

async function type(port, session, target, text) {
  await webdriver(
    port,
    "POST",
    `/session/${session}/element/${target}/value`,
    { text, value: Array.from(text) },
    { phase: "type the native smoke prompt" },
  )
}

async function execute(
  port,
  session,
  script,
  args = [],
  phase = "execute a native window assertion",
) {
  return webdriver(
    port,
    "POST",
    `/session/${session}/execute/sync`,
    { script, args },
    { phase },
  )
}

const directory = mkdtempSync(join(tmpdir(), "nessa-native-window-"))
const data = join(directory, "data")
const configHome = join(directory, "config")
const home = join(directory, "home")
const workspace = join(directory, "workspace")
for (const path of [data, configHome, home, workspace])
  mkdirSync(path, { recursive: true, mode: 0o700 })

const instance = `native-smoke-${process.pid}-${randomUUID().slice(0, 8)}`
const gatewayPort = await freePort()
const driverPort = await freePort()
const nativePort = await freePort()
const providerPath = join(root, "scripts/desktop/native-smoke-provider.mjs")
const modelId = "gpt-5.6-luna"
const sourceCatalog = JSON.parse(
  readFileSync(join(root, "crates/nessa-sdk/data/models.json"), "utf8"),
)
const sourceModel = sourceCatalog.models.find(
  (model) => model.provider === "openai" && model.modelId === modelId,
)
assert.ok(sourceModel, "native smoke model must remain in the bundled catalog")
const catalogPath = join(directory, "models.json")
writeFileSync(
  catalogPath,
  JSON.stringify({
    verifiedOn: sourceCatalog.verifiedOn,
    models: [
      {
        ...sourceModel,
        // The external agent is deterministic and accepts this exact image
        // profile. Keep that fixture fact out of the production model catalog.
        imageInput: {
          mediaTypes: ["image/png"],
          maxEncodedBytes: 1_000_000,
          maxEdgePx: 1024,
          manyImagesMaxEdgePx: 1024,
          nativeLongEdgePx: 1024,
        },
      },
    ],
  }),
  { mode: 0o600 },
)
const configPath = join(data, "ci", "instances", instance, "config.json")
mkdirSync(dirname(configPath), { recursive: true, mode: 0o700 })
writeFileSync(
  configPath,
  JSON.stringify({
    agents: {
      catalog: catalogPath,
      workspace,
      selected: "codex",
      runtimes: {
        codex: {
          command: process.execPath,
          args: [providerPath],
          model: modelId,
          toolsEnabled: true,
        },
      },
    },
  }),
  { mode: 0o600 },
)

const settingsPath = join(configHome, "so.nessa.app", `ci-${instance}`, "settings.json")
mkdirSync(dirname(settingsPath), { recursive: true, mode: 0o700 })
writeFileSync(settingsPath, JSON.stringify({ onboarding: { completed: true } }), {
  mode: 0o600,
})

const buildEnv = {
  ...process.env,
  NESSA_STAGE: "ci",
  VITE_NESSA_STAGE: "ci",
  VITE_NESSA_CONVERSATION_BACKEND: "local",
  VITE_NESSA_GATEWAY_URL: `http://127.0.0.1:${gatewayPort}`,
  TAURI_CONFIG: JSON.stringify({ app: { security: { csp: embeddedCsp(gatewayPort) } } }),
}

const runtimeEnv = {
  ...process.env,
  HOME: home,
  XDG_CONFIG_HOME: configHome,
  NESSA_DATA_DIR: data,
  NESSA_INSTANCE: instance,
  NESSA_PORT: String(gatewayPort),
  NESSA_STAGE: "ci",
  WEBKIT_DISABLE_DMABUF_RENDERER: "1",
  WEBKIT_DISABLE_COMPOSITING_MODE: "1",
}

const logs = []
const failureArtifactRoot = process.env.NESSA_NATIVE_SMOKE_ARTIFACTS
const interruption = watchInterruptions()
const appPidPath = join(directory, "app.pid")
const providerPidPath = join(workspace, "native-smoke-provider.pid")
let server
let driver
let session
let appPid
let providerPid
let failure
let passed
let application
let gateway
let lifecyclePhase = "fixture prepared"

function markPhase(phase, details) {
  lifecyclePhase = phase
  const suffix = details ? `: ${details}` : ""
  const entry = `[harness ${new Date().toISOString()}] ${phase}${suffix}\n`
  logs.push(entry)
  console.log(entry.trimEnd())
}

try {
  requireCommand("tauri-driver")
  requireCommand("WebKitWebDriver")
  if (!process.env.DISPLAY)
    throw new Error("DISPLAY is required; run with xvfb-run on headless Linux")

  checked("pnpm", ["build"], { env: buildEnv })
  const build = spawnSync(
    "cargo",
    [
      "build",
      "-p",
      "nessa-app",
      "-p",
      "nessa-server",
      "--message-format=json-render-diagnostics",
    ],
    {
      cwd: root,
      env: buildEnv,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      stdio: ["inherit", "pipe", "inherit"],
    },
  )
  if (build.error) throw build.error
  assert.equal(build.status, 0, `cargo build exited with ${build.status}`)
  const artifacts = builtExecutables(build.stdout)
  application = artifacts.get("nessa-app")
  gateway = artifacts.get("nessa")
  assert.ok(application, "cargo did not report the desktop executable artifact")
  assert.ok(gateway, "cargo did not report the gateway executable artifact")
  assert.ok(existsSync(application), `desktop executable missing at ${application}`)
  assert.ok(existsSync(gateway), `gateway executable missing at ${gateway}`)
  interruption.signal.throwIfAborted()
  for (const argument of ["login_shell", "--test-threads=1"]) {
    const refused = spawnSync(application, [argument], {
      cwd: root,
      env: runtimeEnv,
      encoding: "utf8",
      timeout: 10_000,
    })
    assert.equal(refused.status, 2, refused.stderr)
    assert.match(refused.stderr, /does not accept command-line arguments/)
    assert.match(
      refused.stderr,
      new RegExp(argument.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
    )
  }

  server = spawn(gateway, ["server", "--provision-local"], {
    cwd: root,
    env: runtimeEnv,
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
  })
  capture(server, "gateway", logs)
  await eventually(
    "gateway health",
    async () => {
      if (server.exitCode !== null)
        throw new Error(`gateway exited with ${server.exitCode}`)
      return fetch(`http://127.0.0.1:${gatewayPort}/health`, {
        signal: AbortSignal.any([AbortSignal.timeout(1_000), interruption.signal]),
      }).then((response) => response.ok)
    },
    30_000,
    interruption.signal,
  )
  assert.ok(alive(server.pid), "gateway process must exist before the UI starts")
  markPhase("gateway ready", `pid=${server.pid}`)

  const wrapper = join(directory, "launch-app.sh")
  writeFileSync(
    wrapper,
    `#!/bin/sh\nprintf '%s\\n' "$$" > ${shellQuote(appPidPath)}\nexec ${shellQuote(application)} "$@"\n`,
    { mode: 0o700 },
  )
  chmodSync(wrapper, 0o700)

  driver = spawn(
    "tauri-driver",
    ["--port", String(driverPort), "--native-port", String(nativePort)],
    {
      cwd: root,
      env: {
        ...runtimeEnv,
        WEBKIT_DEBUG: "SessionHost,WebDriverClassic",
      },
      detached: true,
      stdio: ["ignore", "pipe", "pipe"],
    },
  )
  capture(driver, "driver", logs)
  await eventually(
    "Tauri WebDriver status",
    async () => {
      if (driver.exitCode !== null)
        throw new Error(`tauri-driver exited with ${driver.exitCode}`)
      return fetch(`http://127.0.0.1:${driverPort}/status`, {
        signal: AbortSignal.any([AbortSignal.timeout(1_000), interruption.signal]),
      }).then((response) => response.ok)
    },
    30_000,
    interruption.signal,
  )
  markPhase("Tauri WebDriver ready", `pid=${driver.pid}`)

  markPhase("WebDriver session requested")
  const startup = await observeWebdriverStartup({
    createSession: (signal) =>
      webdriver(
        driverPort,
        "POST",
        "/session",
        {
          capabilities: {
            alwaysMatch: { "tauri:options": { application: wrapper } },
          },
        },
        {
          phase: "create WebDriver session",
          lifecycle: "session",
          signal: AbortSignal.any([interruption.signal, signal]),
        },
      ),
    observeApplication: async (signal) => {
      const pid = await eventually(
        "application launch during WebDriver session creation",
        () => {
          if (!existsSync(appPidPath)) return false
          const recorded = Number(readFileSync(appPidPath, "utf8").trim())
          return alive(recorded) && recorded
        },
        webdriverBudgets.session,
        AbortSignal.any([interruption.signal, signal]),
      )
      appPid = pid
      markPhase("application launched", `pid=${appPid}; session startup active`)
      return pid
    },
  })
  appPid = startup.application
  session = startup.session.sessionId
  assert.ok(session, "WebDriver did not return a session id")
  markPhase("WebDriver session ready", `session=${session}`)

  const frame = await eventually(
    "real panel render and gateway connection",
    () =>
      execute(
        driverPort,
        session,
        `const root = document.querySelector('[data-nessa-root]');
         const fallback = document.querySelector('[data-nessa-load-fallback]');
         if (!root || fallback || !document.body.innerText.includes('Connected')) return null;
         const rect = root.getBoundingClientRect(); const style = getComputedStyle(root);
         return { width: innerWidth, height: innerHeight, rootWidth: rect.width,
           rootHeight: rect.height, display: style.display, visibility: style.visibility };`,
        [],
        "inspect connected panel",
      ),
    60_000,
  )
  assert.ok(frame.width >= 400 && frame.height >= 300, JSON.stringify(frame))
  assert.ok(frame.rootWidth > 0 && frame.rootHeight > 0, JSON.stringify(frame))
  assert.notEqual(frame.display, "none")
  assert.notEqual(frame.visibility, "hidden")
  markPhase("connected panel rendered")

  const composer = await element(driverPort, session, '[aria-label="Message"]')
  await type(driverPort, session, composer, `native smoke${String.fromCodePoint(0xe007)}`)
  await eventually("deterministic agent reply", () =>
    execute(
      driverPort,
      session,
      "return document.body.innerText.includes('Smoke reply: native smoke')",
      [],
      "inspect deterministic agent reply",
    ),
  )
  markPhase("deterministic reply rendered")

  await execute(
    driverPort,
    session,
    `const bytes = Uint8Array.from(atob(arguments[0]), c => c.charCodeAt(0));
     const file = new File([bytes], 'native-smoke.png', { type: 'image/png' });
     const transfer = new DataTransfer(); transfer.items.add(file);
     const target = document.querySelector('[data-nessa-root]');
     for (const type of ['dragenter', 'dragover', 'drop'])
       target.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: transfer }));
     return true;`,
    [png],
    "dispatch synthetic image drop",
  )
  const attachment = await eventually("stored image attachment", () =>
    execute(
      driverPort,
      session,
      `const stored = document.querySelector('[data-upload="stored"]');
       const tile = stored?.querySelector('[data-slot="chat-attachment-tile"]');
       const image = tile?.querySelector('img');
       if (!stored || !tile || !image || !image.complete || image.naturalWidth < 1) return null;
       return { upload: stored.dataset.upload, title: tile.title,
         width: image.naturalWidth, height: image.naturalHeight };`,
      [],
      "inspect stored image attachment",
    ),
  )
  assert.deepEqual(attachment, {
    upload: "stored",
    title: "native-smoke.png",
    width: 1,
    height: 1,
  })

  providerPid = await eventually("ACP provider PID", () => {
    if (!existsSync(providerPidPath)) return false
    const pid = Number(readFileSync(providerPidPath, "utf8").trim())
    return alive(pid) && pid
  })
  assert.ok(alive(appPid) && alive(providerPid) && alive(server.pid) && alive(driver.pid))
  markPhase("native smoke assertions complete")
  passed = `gateway=${server.pid}, driver=${driver.pid}, app=${appPid}, provider=${providerPid}`
} catch (error) {
  failure = error
} finally {
  if (session)
    await webdriver(driverPort, "DELETE", `/session/${session}`, undefined, {
      phase: "delete WebDriver session",
      signal: null,
    }).catch((error) => logs.push(`[cleanup] ${error}`))
  appPid ??= recordedPid(appPidPath)
  providerPid ??= recordedPid(providerPidPath)
  const driverGroupGone = await stopOwnedGroup(driver?.pid)
  const serverGroupGone = await stopOwnedGroup(server?.pid)
  const appProcessGone = await stopRecordedProcess(appPid)
  const providerProcessGone = await stopRecordedProcess(providerPid)
  for (const [name, gone] of [
    ["driver process group", driverGroupGone],
    ["gateway process group", serverGroupGone],
    ["application process", appProcessGone],
    ["provider process", providerProcessGone],
  ]) {
    if (gone) continue
    const cleanup = new Error(`${name} cleanup could not be verified`)
    failure = failure ? new AggregateError([failure, cleanup]) : cleanup
  }
  for (const [name, pid] of [
    ["application", appPid],
    ["provider", providerPid],
    ["gateway", server?.pid],
    ["driver", driver?.pid],
  ]) {
    if (!pid) continue
    const gone = await eventually(
      `${name} cleanup`,
      () => !alive(pid),
      10_000,
      null,
    ).catch(() => false)
    if (!gone) {
      const cleanup = new Error(`${name} process ${pid} survived cleanup`)
      failure = failure ? new AggregateError([failure, cleanup]) : cleanup
    }
  }
  if (failure) {
    let failureText = renderNativeSmokeError(failure)
    logs.push(`[failure] ${failureText}\n`)
    console.error(logs.join("").slice(-12_000))
    console.error(`native smoke failure:\n${failureText}`)
    if (failureArtifactRoot) {
      try {
        const retained = retainNativeSmokeFailure(failureArtifactRoot, instance, {
          logs: logs.join(""),
          metadata: {
            lifecyclePhase,
            error: failureText,
            instance,
            ports: { gateway: gatewayPort, driver: driverPort, native: nativePort },
            pids: {
              gateway: server?.pid,
              driver: driver?.pid,
              application: appPid,
              provider: providerPid,
            },
            sessionCreated: Boolean(session),
            executableArtifacts: { application, gateway },
          },
        })
        console.error(`native smoke failure artifacts: ${retained}`)
      } catch (artifactError) {
        failure = new AggregateError([failure, artifactError])
        failureText = renderNativeSmokeError(failure)
        console.error(`could not retain native smoke failure artifacts:\n${failureText}`)
      }
    }
  }
  rmSync(directory, { recursive: true, force: true })
  interruption.dispose()
}

if (failure) throw failure
console.log(`native-window smoke passed and cleaned up (${passed})`)
