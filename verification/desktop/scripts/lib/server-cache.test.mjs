/** Real Vite/React cache contamination and opt-in process/cache ownership. */
import { strict as assert } from "node:assert"
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath, pathToFileURL } from "node:url"

const lib = dirname(fileURLToPath(import.meta.url))
const repo = join(lib, "../../../..")

async function fixture() {
  const root = mkdtempSync(join(tmpdir(), "nessa-vite-cache-test-"))
  const copied = join(root, "verification/desktop/scripts/lib")
  mkdirSync(copied, { recursive: true })
  for (const file of ["server.mjs", "dev-server.mjs", "cli.mjs", "page-lines.mjs"])
    copyFileSync(join(lib, file), join(copied, file))
  symlinkSync(join(repo, "node_modules"), join(root, "node_modules"), "junction")
  writeFileSync(join(root, "package.json"), '{"type":"module"}')
  writeFileSync(
    join(root, "desktop.html"),
    '<script type="module" src="/entry.js"></script>',
  )
  writeFileSync(
    join(root, "entry.js"),
    'import { jsxDEV } from "react/jsx-dev-runtime"; globalThis.jsxDEV = jsxDEV;',
  )
  // Keep the default cache inside this disposable fixture even though its
  // dependencies are linked. The same config serves both CLI and inline paths.
  writeFileSync(
    join(root, "vite.config.mjs"),
    `import { existsSync, writeFileSync } from "node:fs"
let cacheDir
export default {
  cacheDir: ${JSON.stringify(join(root, "shared-cache"))},
  optimizeDeps: { include: ["react/jsx-dev-runtime"] },
  plugins: [{ name: "cache-owner-evidence", configResolved(config) {
    cacheDir = config.cacheDir
    writeFileSync(${JSON.stringify(join(root, "launch.json"))}, JSON.stringify({
      cacheDir: config.cacheDir, pid: process.pid,
      production: config.isProduction,
    }))
    if (process.env.CACHE_TEST_FAIL === "1") throw new Error("fixture startup failure")
  }, async closeBundle() {
    if (process.env.CACHE_TEST_DELAY_EXIT === "1") {
      await new Promise(resolve => setTimeout(resolve, 150))
        writeFileSync(${JSON.stringify(join(root, "exit.json"))}, JSON.stringify({
          cacheExistsBeforeExit: existsSync(cacheDir),
        }))
    }
  }}],
}`,
  )
  const { startDevServer } = await import(pathToFileURL(join(copied, "server.mjs")))
  const servers = []
  return {
    root,
    async start(options, env) {
      const server = await startDevServer(options, env)
      servers.push(server)
      const evidence = JSON.parse(readFileSync(join(root, "launch.json"), "utf8"))
      return { ...server, ...evidence }
    },
    async close() {
      for (const server of servers.reverse()) await server.close()
      rmSync(root, { recursive: true, force: true })
    },
  }
}

function runtimePath(server) {
  return join(server.cacheDir, "deps/react_jsx-dev-runtime.js")
}

async function runtime(server, version) {
  const url = pathToFileURL(runtimePath(server))
  url.searchParams.set("revision", version)
  const module = await import(url.href)
  return module.default ?? module
}

test("shared production optimization overwrites the development JSX runtime", async () => {
  const f = await fixture()
  try {
    const canonical = await f.start({}, { NODE_ENV: "development" })
    const before = readFileSync(runtimePath(canonical), "utf8")
    assert.equal(typeof (await runtime(canonical, "before")).jsxDEV, "function")
    const capture = await f.start({}, { NODE_ENV: "production" })
    assert.equal(capture.cacheDir, canonical.cacheDir)
    assert.equal(capture.production, true)
    assert.notEqual(readFileSync(runtimePath(canonical), "utf8"), before)
    assert.equal((await runtime(canonical, "overwritten")).jsxDEV, undefined)
  } finally {
    await f.close()
  }
})

test("isolated optimizers preserve the concurrent canonical cache and runtime", async () => {
  const f = await fixture()
  try {
    const canonical = await f.start({}, { NODE_ENV: "development" })
    const before = readFileSync(runtimePath(canonical), "utf8")
    const production = await f.start({ isolateCache: true }, { NODE_ENV: "production" })
    const capture = await f.start({ isolateCache: true }, { NODE_ENV: "development" })
    assert.notEqual(production.cacheDir, canonical.cacheDir)
    assert.notEqual(capture.cacheDir, canonical.cacheDir)
    assert.notEqual(capture.cacheDir, production.cacheDir)
    assert.equal((await runtime(production, "isolated")).jsxDEV, undefined)
    assert.equal(typeof (await runtime(capture, "isolated")).jsxDEV, "function")
    assert.equal(readFileSync(runtimePath(canonical), "utf8"), before)
    const jsxDEV = (await runtime(canonical, "preserved")).jsxDEV
    assert.equal(typeof jsxDEV, "function")
    assert.equal(jsxDEV("div", {}, undefined, false).type, "div")
    await production.close()
    await capture.close()
    assert.equal(existsSync(production.cacheDir), false)
    assert.equal(existsSync(capture.cacheDir), false)
    assert.equal(existsSync(canonical.cacheDir), true)
  } finally {
    await f.close()
  }
})

test("isolated cache survives until the owned process exits, then close is repeatable", async () => {
  const f = await fixture()
  try {
    const server = await f.start(
      { isolateCache: true },
      { NODE_ENV: "development", CACHE_TEST_DELAY_EXIT: "1" },
    )
    await server.close()
    assert.deepEqual(JSON.parse(readFileSync(join(f.root, "exit.json"), "utf8")), {
      cacheExistsBeforeExit: true,
    })
    assert.throws(() => process.kill(server.pid, 0), { code: "ESRCH" })
    assert.equal(existsSync(server.cacheDir), false)
    await server.close()
  } finally {
    await f.close()
  }
})

test("configuration failure stops its child and removes the private cache", async () => {
  const f = await fixture()
  try {
    await assert.rejects(
      f.start({ isolateCache: true }, { CACHE_TEST_FAIL: "1" }),
      /fixture startup failure/,
    )
    const launch = JSON.parse(readFileSync(join(f.root, "launch.json"), "utf8"))
    assert.equal(existsSync(launch.cacheDir), false)
    assert.throws(() => process.kill(launch.pid, 0), { code: "ESRCH" })
  } finally {
    await f.close()
  }
})
