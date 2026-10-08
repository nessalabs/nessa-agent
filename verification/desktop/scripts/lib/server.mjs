/**
 * The page under test: an explicit --url, the Vite dev server (reused when
 * it already answers on 127.0.0.1:1438, started otherwise, and warmed either
 * way), or a
 * production build previewed on a free port. The performance budget is
 * stated for a production build, so perf numbers from dev are indicative
 * only.
 */
import { spawn } from "node:child_process"
import { mkdtempSync, rmSync } from "node:fs"
import { createServer } from "node:net"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { CannotRun, log } from "./cli.mjs"

export const repoRoot = resolve(fileURLToPath(import.meta.url), "../../../../..")
const vite = join(repoRoot, "node_modules/.bin/vite")
const devUrl = "http://127.0.0.1:1438/desktop.html"

async function answers(url) {
  try {
    const response = await fetch(url, { signal: AbortSignal.timeout(2000) })
    return response.ok
  } catch {
    return false
  }
}

async function waitFor(url, ms, child) {
  const until = Date.now() + ms
  while (Date.now() < until) {
    if (child && child.exitCode !== null) return false
    if (await answers(url)) return true
    await new Promise((r) => setTimeout(r, 300))
  }
  return false
}

export function freePort() {
  return new Promise((ok, fail) => {
    const server = createServer()
    server.once("error", fail)
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address()
      server.close(() => ok(port))
    })
  })
}

/**
 * Has the dev server at `url` transform the page's module graph before any
 * page asks for it: the page's module scripts, then every module they import
 * by path, as Vite serves them. A cold server otherwise does that work inside
 * the first page's ready wait. Best effort and bounded: a module that does not
 * answer is left for the page to ask for, and a page that never renders is
 * still the page's ready wait's to say — this decides nothing.
 */
async function warm(url, ms = 120_000) {
  const started = Date.now()
  const until = started + ms
  const origin = new URL(url).origin
  const seen = new Set()
  const fetchText = async (path) => {
    try {
      const response = await fetch(new URL(path, origin), {
        signal: AbortSignal.timeout(Math.max(1, until - Date.now())),
      })
      return response.ok ? await response.text() : ""
    } catch {
      return ""
    }
  }
  // What a module, or the page, names by an absolute path: Vite rewrites
  // every import, relative or bare, to one.
  const named = (text) =>
    [
      ...text.matchAll(/\bsrc="(\/[^"]+)"/g),
      ...text.matchAll(/(?:\bfrom|\bimport)\s*\(?\s*["'](\/[^"']+)["']/g),
    ].map((match) => match[1])
  let next = named(await fetchText(new URL(url).pathname))
  while (next.length > 0 && Date.now() < until) {
    const fresh = [...new Set(next)].filter((path) => !seen.has(path))
    fresh.forEach((path) => seen.add(path))
    next = []
    for (const text of await Promise.all(fresh.map(fetchText))) next.push(...named(text))
  }
  log(`warmed ${seen.size} modules in ${Date.now() - started} ms`)
}

/** Runs a child with its output kept off stdout; the tail is kept for errors. */
function child(command, args, options, env = process.env) {
  const proc = spawn(command, args, {
    cwd: repoRoot,
    stdio: ["ignore", "pipe", "pipe"],
    env,
  })
  const tail = []
  const keep = (chunk) => {
    const text = chunk.toString()
    if (options.verbose) process.stderr.write(text)
    tail.push(text)
    if (tail.length > 40) tail.shift()
  }
  proc.stdout.on("data", keep)
  proc.stderr.on("data", keep)
  proc.tail = () => tail.join("")
  return proc
}

/** The child has exited. A signal leaves `exitCode` null, so that alone is not enough. */
function exited(proc) {
  return new Promise((ok) =>
    proc.exitCode !== null || proc.signalCode !== null
      ? ok(proc.exitCode)
      : proc.once("exit", ok),
  )
}

/**
 * Starts a dev server of its own on a free port, with `env` added to its
 * environment — never reusing one already running, whose environment is not
 * this caller's (a check that points the dev server's `/browser` proxy at a
 * gateway of its own, `NESSA_BROWSER_GATEWAY_URL`). Returns `{ url, close }`.
 */
export async function startDevServer(options, env) {
  const port = await freePort()
  log(`starting a dev server of its own on port ${port}…`)
  return devServer(port, options, { ...process.env, ...env })
}

/**
 * Starts the Vite dev server on `port` and resolves once it answers and has
 * been warmed (`warm`): every dev server a check starts is started here.
 * Returns `{ url, close }`.
 */
async function devServer(port, options, env = process.env) {
  const url = `http://127.0.0.1:${port}/desktop.html`
  const proc = child(
    vite,
    ["--host", "127.0.0.1", "--port", String(port), "--strictPort"],
    options,
    env,
  )
  if (!(await waitFor(url, 60_000, proc))) {
    proc.kill()
    await exited(proc)
    throw new CannotRun(`dev server did not answer at ${url}\n${proc.tail()}`)
  }
  await warm(url)
  return {
    url,
    close: async () => {
      proc.kill()
      await exited(proc)
    },
  }
}

/**
 * Stops a preview, if one is running, and removes its directory. The process
 * has exited before the directory goes: a build that failed never started
 * one, and a preview that did not answer is killed here and waited on.
 */
export async function stopPreview(outDir, preview) {
  try {
    if (preview && preview.exitCode === null && preview.signalCode === null) {
      preview.kill()
      await exited(preview)
    }
  } finally {
    rmSync(outDir, { recursive: true, force: true })
  }
}

/**
 * Build environment for `run-all --mode prod`'s functional preview.
 *
 * The build stays a minified production bundle (`vite build`). Both stage
 * halves are `ci` — the same explicit stage the gateway-backed production
 * verifier passes to `startPreview` — because the stage is inlined at build
 * time and a prod stage refuses a scripted numeric-loopback browser socket.
 * The perf budget does not use this: its own preview keeps the default prod
 * stage.
 */
export function functionalPreviewEnv() {
  return { NESSA_STAGE: "ci", VITE_NESSA_STAGE: "ci" }
}

/**
 * Builds a production bundle and previews it on a free port. `env` is added
 * to the build and the preview: a check that talks to a ci gateway sets
 * `VITE_NESSA_STAGE=ci`, because the stage is inlined at build time and a
 * prod stage refuses that gateway's HTTP loopback address. Returns
 * `{ url, mode, close }`. An optional `configPath` selects a verification-only
 * build configuration, without changing the packaged application inputs.
 * A build that fails, or a preview that does not
 * answer, removes that directory after the preview process has exited.
 */
export async function startPreview(options, env = {}, configPath) {
  const configArgs = configPath ? ["--config", configPath] : []
  const outDir = mkdtempSync(join(tmpdir(), "nessa-desktop-verify-"))
  let preview
  try {
    log(`building production into ${outDir}…`)
    const buildEnv = { ...process.env, ...env }
    const build = child(
      vite,
      ["build", ...configArgs, "--outDir", outDir, "--emptyOutDir"],
      options,
      buildEnv,
    )
    if ((await exited(build)) !== 0)
      throw new CannotRun(`vite build failed\n${build.tail()}`)
    const port = await freePort()
    const url = `http://127.0.0.1:${port}/desktop.html`
    preview = child(
      vite,
      [
        "preview",
        ...configArgs,
        "--outDir",
        outDir,
        "--host",
        "127.0.0.1",
        "--port",
        String(port),
        "--strictPort",
      ],
      options,
      buildEnv,
    )
    if (!(await waitFor(url, 30_000, preview)))
      throw new CannotRun(`vite preview did not answer at ${url}\n${preview.tail()}`)
    log(`previewing production at ${url}`)
    const running = preview
    preview = undefined
    return {
      url,
      mode: "prod",
      close: () => stopPreview(outDir, running),
    }
  } catch (error) {
    await stopPreview(outDir, preview)
    throw error
  }
}

/**
 * Resolves the page to test. Returns `{ url, mode, close }`; `close` stops
 * anything this call started and leaves a reused server running.
 */
/**
 * The mode a check's body should see. `--url` skips starting a server. An
 * explicit `--mode dev|prod` beside it still names which page that url is,
 * so a production run can leave dev-server steps out. Without an explicit
 * mode, the url is `given`.
 */
export function pageMode(options) {
  if (!options.url) return options.mode
  if (
    typeof options.given === "function" &&
    options.given("mode") &&
    (options.mode === "dev" || options.mode === "prod")
  )
    return options.mode
  return "given"
}

export async function target(options, env = {}) {
  if (options.url)
    return { url: options.url, mode: pageMode(options), close: async () => {} }

  if (options.mode === "dev") {
    if (await answers(devUrl)) {
      log(`using the running dev server at ${devUrl}`)
      // Warmed as one this call starts is: a server nobody has loaded a page
      // from yet is as cold, and on a warm one this is quick.
      await warm(devUrl)
      return { url: devUrl, mode: "dev", close: async () => {} }
    }
    log("starting the dev server (pnpm desktop:dev)…")
    const dev = await devServer(new URL(devUrl).port, options)
    return { url: dev.url, mode: "dev", close: dev.close }
  }

  if (options.mode === "prod") return startPreview(options, env)

  throw new CannotRun(`unknown --mode ${options.mode} (dev or prod)`)
}
