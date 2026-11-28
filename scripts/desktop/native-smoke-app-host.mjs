/**
 * Runs `nessa-app-host`, the process each installed app runs in as an app of
 * its own, on Linux WebKitGTK under a virtual display:
 *
 * - it draws the package's own files and its page may read them, while a
 *   request, an image or a window to a local server never leaves (the server
 *   sees nothing), and the page has no WebRTC;
 * - while it runs it holds the launcher's running lock and names its process;
 *   a second launch leaves at once, and SIGTERM (Stop) ends it and frees the
 *   lock;
 * - a launcher whose bytes changed opens nothing;
 * - a dynamic app's server is run with node beside it, its page reaches that
 *   server with the launch's token and still reaches nothing else, and the
 *   server ends with the host; one whose launcher holds no credential says
 *   so in its window.
 *
 * The launcher is written here as `nessa-app-package`'s `launcher_record`
 * writes it; that crate's tests hold the writer to the reader.
 */
import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import { createHash } from "node:crypto"
import { once } from "node:events"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { createServer } from "node:http"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { setTimeout as delay } from "node:timers/promises"
import { builtExecutables } from "./native-smoke-build.mjs"
import {
  OwnedCommandCleanupError,
  runOwnedCommand,
  watchInterruptions,
} from "./native-smoke-processes.mjs"

const APP = "smoke-app"
const HEADING = "Drawn from the launcher's bytes"

/** One launcher record, as `launcher_record` writes it. */
export function launcherRecord(appId, files, server) {
  const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex")
  const paths = Object.keys(files).sort()
  const manifest = Buffer.from(
    JSON.stringify({
      schemaVersion: 1,
      appId,
      packageVersion: "1.0.0",
      entrypoint: "index.html",
      ...(server ? { server } : {}),
      // A server signs in as the app, so a dynamic app asks for something.
      requestedCapabilities: server ? ["list"] : [],
      assets: paths.map((path) => ({
        path,
        sizeBytes: files[path].length,
        sha256: sha256(files[path]),
      })),
    }),
  )
  return JSON.stringify({
    recordVersion: 1,
    appId,
    digest: sha256(manifest),
    manifest: manifest.toString("base64"),
    assets: paths.map((path) => ({ path, bytes: files[path].toString("base64") })),
  })
}

/**
 * A dynamic app: a page that asks its server, synchronously so the probe
 * sees the answer, and a server that answers only this launch's token.
 */
const DYNAMIC_PAGE = `<!doctype html><title>Dynamic app</title><h1>no answer</h1><script>
  const request = new XMLHttpRequest()
  request.open("GET", window.nessaApp.server + "/hello", false)
  request.setRequestHeader("x-nessa-app-token", window.nessaApp.token)
  request.send()
  document.querySelector("h1").textContent = request.responseText
</script>`
const DYNAMIC_SERVER = `import { createServer } from "node:http"
import { createHash } from "node:crypto"
import { writeFileSync } from "node:fs"
writeFileSync("pid", String(process.pid))
const ready = createHash("sha256").update(process.env.NESSA_APP_TOKEN).digest("hex")
createServer((request, response) => {
  if (request.url === "/.nessa-ready") return response.end(ready)
  response.setHeader("access-control-allow-origin", "*")
  response.setHeader("access-control-allow-headers", "x-nessa-app-token")
  if (request.method === "OPTIONS") return response.writeHead(204).end()
  if (request.headers["x-nessa-app-token"] !== process.env.NESSA_APP_TOKEN)
    return response.writeHead(401).end("refused")
  response.end("answered by " + process.env.NESSA_APP_ID)
}).listen(Number(process.env.NESSA_APP_PORT), process.env.NESSA_APP_HOST)
`

async function dynamicApp({ host, directory, env, signal, leak, hits }) {
  const folder = join(directory, "dynamic")
  mkdirSync(folder)
  writeFileSync(
    join(folder, "app.json"),
    launcherRecord(
      "smoke-dynamic",
      {
        "index.html": Buffer.from(DYNAMIC_PAGE),
        "server/main.mjs": Buffer.from(DYNAMIC_SERVER),
      },
      { entry: "server/main.mjs", runtime: "node" },
    ),
  )
  const probe = async () => {
    const probed = await runOwnedCommand(host, ["--app", folder], {
      cwd: directory,
      env: { ...env, NESSA_APP_HOST_PROBE: leak },
      signal,
      timeout: 60_000,
      captureLimit: 1024 * 1024,
    })
    const line = probed
      .split("\n")
      .find((each) => each.startsWith("nessa-app-host probe: "))
    assert.ok(line, `the host reported no probe: ${probed}`)
    return JSON.parse(line.slice("nessa-app-host probe: ".length))
  }

  // No credential kept: the window says why, and no server runs.
  const unkept = await probe()
  assert.equal(unkept.heading, "smoke-dynamic could not start")
  console.log("app host smoke: a dynamic app with no credential said so")

  writeFileSync(
    join(folder, "credential.json"),
    JSON.stringify({ credential: "unused-here", stage: "dev" }),
    { mode: 0o600 },
  )
  const seen = await probe()
  assert.equal(seen.heading, "answered by smoke-dynamic", JSON.stringify(seen))
  assert.equal(seen.reached, false, "the dynamic app's page reached past its server")
  assert.equal(hits(), 0, "a request from the dynamic app reached the local server")
  // Its server ended with the host.
  const pid = Number(readFileSync(join(folder, "server", "pid"), "utf8"))
  await until(() => {
    try {
      process.kill(pid, 0)
      return false
    } catch {
      return true
    }
  }, "the app's server to end with its host")
  console.log(
    "app host smoke: a dynamic app's page reached its own server, and nothing else",
  )
}

/** Whether another process holds `lock` (util-linux `flock`). */
function held(lock) {
  return spawnSync("flock", ["--nonblock", lock, "true"]).status !== 0
}

async function until(check, what, timeout = 20_000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    if (check()) return
    await delay(100)
  }
  throw new Error(`timed out waiting for ${what}`)
}

export async function runAppHostSmoke(root) {
  assert.equal(process.platform, "linux", "app host smoke requires Linux")
  assert.ok(process.env.DISPLAY, "run app host smoke with xvfb-run")
  const directory = mkdtempSync(join(tmpdir(), "nessa-app-host-"))
  const interruption = watchInterruptions()
  const { signal } = interruption
  const env = {
    ...process.env,
    HOME: directory,
    XDG_CONFIG_HOME: join(directory, "config"),
    XDG_DATA_HOME: join(directory, "data"),
    XDG_CACHE_HOME: join(directory, "cache"),
    WEBKIT_DISABLE_DMABUF_RENDERER: "1",
    WEBKIT_DISABLE_COMPOSITING_MODE: "1",
  }
  let hits = 0
  const server = createServer((_, response) => {
    hits += 1
    response.end("reached")
  })
  let running
  let failure
  try {
    server.listen(0, "127.0.0.1")
    await once(server, "listening")
    const leak = `http://127.0.0.1:${server.address().port}/leak`

    const output = await runOwnedCommand(
      "cargo",
      [
        "build",
        "--locked",
        "-p",
        "nessa-app",
        "--no-default-features",
        "--bin",
        "nessa-app-host",
        "--message-format=json-render-diagnostics",
      ],
      { cwd: root, signal, timeout: 12 * 60_000, captureLimit: 64 * 1024 * 1024 },
    )
    const host = builtExecutables(output).get("nessa-app-host")
    assert.ok(host, "Cargo did not report nessa-app-host")

    const folder = join(directory, "launcher")
    mkdirSync(folder)
    const record = launcherRecord(APP, {
      "index.html": Buffer.from(
        `<!doctype html><title>Smoke app</title><link rel="stylesheet" href="style.css"><h1>${HEADING}</h1>`,
      ),
      "style.css": Buffer.from("h1 { font: 24px sans-serif }"),
    })
    writeFileSync(join(folder, "app.json"), record)

    // Drawn from its own bytes, which it may read; nothing gets out.
    const probed = await runOwnedCommand(host, ["--app", folder], {
      cwd: directory,
      env: { ...env, NESSA_APP_HOST_PROBE: leak },
      signal,
      timeout: 60_000,
      captureLimit: 1024 * 1024,
    })
    const line = probed
      .split("\n")
      .find((each) => each.startsWith("nessa-app-host probe: "))
    assert.ok(line, `the host reported no probe: ${probed}`)
    const seen = JSON.parse(line.slice("nessa-app-host probe: ".length))
    assert.deepEqual(seen, {
      title: "Smoke app",
      heading: HEADING,
      reached: false,
      own: true,
      rtc: false,
      opened: false,
    })
    assert.equal(hits, 0, "a request from the app reached the local server")
    assert.equal(held(join(folder, "running.lock")), false, "the lock outlived the app")
    console.log("app host smoke: drew its own bytes, read its own file, reached nothing")

    // Running holds the lock and names its process; a second launch leaves.
    running = spawn(host, ["--app", folder], { cwd: directory, env, stdio: "ignore" })
    const exited = once(running, "exit")
    await until(() => held(join(folder, "running.lock")), "the running lock")
    assert.equal(
      Number(readFileSync(join(folder, "running.pid"), "utf8")),
      running.pid,
      "the running pid is not the app's",
    )
    await runOwnedCommand(host, ["--app", folder], {
      cwd: directory,
      env,
      signal,
      timeout: 20_000,
    })
    assert.equal(running.exitCode, null, "a second launch ended the first")
    console.log("app host smoke: one process per app; a second launch left at once")

    // Stop is SIGTERM to the named process; the lock goes with it.
    process.kill(running.pid, "SIGTERM")
    await exited
    running = undefined
    await until(() => !held(join(folder, "running.lock")), "the lock to be freed")
    console.log("app host smoke: Stop ended it and freed its lock")

    // A launcher whose bytes changed opens nothing.
    const changed = JSON.parse(record)
    changed.assets[0].bytes = Buffer.from("<h1>changed</h1>").toString("base64")
    writeFileSync(join(folder, "app.json"), JSON.stringify(changed))
    await assert.rejects(
      runOwnedCommand(host, ["--app", folder], {
        cwd: directory,
        env,
        signal,
        timeout: 20_000,
      }),
      /exited with 1/,
    )
    assert.equal(held(join(folder, "running.lock")), false)
    console.log("app host smoke: changed bytes opened nothing")

    await dynamicApp({ host, directory, env, signal, leak, hits: () => hits })
  } catch (error) {
    failure = error
  } finally {
    if (running?.exitCode === null) running.kill("SIGKILL")
    server.close()
    try {
      if (!(failure instanceof OwnedCommandCleanupError))
        rmSync(directory, { recursive: true, force: true })
    } catch (cleanup) {
      failure = failure ? new AggregateError([failure, cleanup]) : cleanup
    } finally {
      interruption.dispose()
    }
  }
  if (failure) throw failure
  console.log("native app host smoke passed and cleaned up")
}
