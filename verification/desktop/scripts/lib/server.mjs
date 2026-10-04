/**
 * The page under test: an explicit --url, the Vite dev server (reused when
 * it already answers on 127.0.0.1:1438, started otherwise), or a production
 * build previewed on a free port. The performance budget is stated for a
 * production build, so perf numbers from dev are indicative only.
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

function exited(proc) {
  return new Promise((ok) =>
    proc.exitCode !== null ? ok(proc.exitCode) : proc.once("exit", ok),
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
  const url = `http://127.0.0.1:${port}/desktop.html`
  log(`starting a dev server of its own at ${url}…`)
  const proc = child(
    vite,
    ["--host", "127.0.0.1", "--port", String(port), "--strictPort"],
    options,
    { ...process.env, ...env },
  )
  if (!(await waitFor(url, 60_000, proc))) {
    proc.kill()
    await exited(proc)
    throw new CannotRun(`dev server did not answer at ${url}\n${proc.tail()}`)
  }
  return {
    url,
    close: async () => {
      proc.kill()
      await exited(proc)
    },
  }
}

/**
 * Resolves the page to test. Returns `{ url, mode, close }`; `close` stops
 * anything this call started and leaves a reused server running.
 */
export async function target(options) {
  if (options.url) return { url: options.url, mode: "given", close: async () => {} }

  if (options.mode === "dev") {
    if (await answers(devUrl)) {
      log(`using the running dev server at ${devUrl}`)
      return { url: devUrl, mode: "dev", close: async () => {} }
    }
    log("starting the dev server (pnpm desktop:dev)…")
    const proc = child(
      vite,
      ["--host", "127.0.0.1", "--port", "1438", "--strictPort"],
      options,
    )
    if (!(await waitFor(devUrl, 60_000, proc))) {
      proc.kill()
      throw new CannotRun(`dev server did not answer at ${devUrl}\n${proc.tail()}`)
    }
    return {
      url: devUrl,
      mode: "dev",
      close: async () => {
        proc.kill()
        await exited(proc)
      },
    }
  }

  if (options.mode === "prod") {
    const outDir = mkdtempSync(join(tmpdir(), "nessa-desktop-verify-"))
    log(`building production into ${outDir}…`)
    const build = child(vite, ["build", "--outDir", outDir, "--emptyOutDir"], options)
    if ((await exited(build)) !== 0)
      throw new CannotRun(`vite build failed\n${build.tail()}`)
    const port = await freePort()
    const url = `http://127.0.0.1:${port}/desktop.html`
    const preview = child(
      vite,
      [
        "preview",
        "--outDir",
        outDir,
        "--host",
        "127.0.0.1",
        "--port",
        String(port),
        "--strictPort",
      ],
      options,
    )
    if (!(await waitFor(url, 30_000, preview))) {
      preview.kill()
      throw new CannotRun(`vite preview did not answer at ${url}\n${preview.tail()}`)
    }
    log(`previewing production at ${url}`)
    return {
      url,
      mode: "prod",
      close: async () => {
        preview.kill()
        await exited(preview)
        rmSync(outDir, { recursive: true, force: true })
      },
    }
  }

  throw new CannotRun(`unknown --mode ${options.mode} (dev or prod)`)
}
