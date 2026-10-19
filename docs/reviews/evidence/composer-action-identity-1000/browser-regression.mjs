/** Synthetic real-component regression. Never starts a gateway or reads a profile. */
import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { mkdtemp, readFile, realpath, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { createServer } from "vite"
import react from "@vitejs/plugin-react"
import tailwindcss from "@tailwindcss/vite"
import { chromium, webkit } from "playwright"
import { sharedPackages, viteAliases } from "../../../../scripts/nessa-ui-paths.mjs"

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..")
const ui = resolve(
  await realpath(join(repository, "node_modules/@nessa-ui/react")),
  "src",
)
const component = join(repository, "src/desktop/ui/composer.tsx")
const hash = async () =>
  createHash("sha256")
    .update(await readFile(component))
    .digest("hex")
const before = await hash()
const cache = await mkdtemp(join(tmpdir(), "nessa-composer-regression-"))
const results = []
let server
let failed = false
try {
  server = await createServer({
    configFile: false,
    envFile: false,
    root: repository,
    publicDir: false,
    cacheDir: cache,
    logLevel: "error",
    plugins: [
      react(),
      tailwindcss(),
      {
        name: "synthetic-composer-fixture",
        configureServer(vite) {
          vite.middlewares.use(
            "/__composer-regression",
            async (_request, response, next) => {
              try {
                response.setHeader("Content-Type", "text/html")
                response.end(
                  await vite.transformIndexHtml(
                    "/__composer-regression",
                    '<!doctype html><html><head><link rel="icon" href="data:,"></head><body><div id="root" style="width:880px;padding:40px"></div><script type="module" src="/docs/reviews/evidence/composer-action-identity-1000/browser-fixture.tsx"></script></body></html>',
                  ),
                )
              } catch (error) {
                next(error)
              }
            },
          )
        },
      },
    ],
    resolve: { alias: viteAliases(ui), dedupe: [...sharedPackages] },
    server: { host: "127.0.0.1", port: 0, fs: { allow: [repository, dirname(ui)] } },
  })
  await server.listen()
  const address = server.httpServer.address()
  assert.ok(address && typeof address === "object")
  const origin = `http://127.0.0.1:${address.port}`
  for (const [engine, type] of Object.entries({ chromium, webkit })) {
    let browser
    try {
      browser = await type.launch({ headless: true })
      const page = await browser.newPage({ viewport: { width: 1000, height: 800 } })
      const errors = []
      page.on("pageerror", (error) => errors.push(error.message))
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text())
      })
      page.on("requestfailed", (request) =>
        errors.push(`${request.url()}: ${request.failure()?.errorText}`),
      )
      await page.route("**/*", (route) => {
        const url = new URL(route.request().url())
        return url.origin === origin || url.protocol === "data:"
          ? route.continue()
          : route.abort()
      })
      await page.goto(`${origin}/__composer-regression`)
      await page.waitForFunction(() => window.composerRegression?.ready())
      await page.evaluate(() => document.fonts.ready)
      const button = page.locator(".desktop-send")
      const textarea = page.locator("textarea")
      const mode = (value) =>
        page.evaluate((value) => window.composerRegression.mode(value), value)
      async function check(name, initial, action, sends, stops, canceled = false) {
        await page.evaluate((value) => window.composerRegression.reset(value), initial)
        const geometry = await action()
        const actual = await page.evaluate(() => window.composerRegression.result())
        assert.deepEqual(
          actual.sent,
          sends ? ["SYNTHETIC unsent draft"] : [],
          `${engine} ${name}: sends`,
        )
        assert.equal(actual.stops, stops, `${engine} ${name}: stops`)
        assert.equal(
          actual.draft,
          sends ? "" : "SYNTHETIC unsent draft",
          `${engine} ${name}: draft`,
        )
        if (canceled)
          assert.equal(actual.focus, "BODY", `${engine} ${name}: canceled focus`)
        results.push({ engine, name, ...actual, geometry })
      }
      async function pointer(geometry, transitions) {
        const rect = await button.boundingBox()
        const icon = await button.locator("svg").boundingBox()
        assert.ok(rect && icon)
        const point = {
          x: rect.x + (geometry === "padding" ? 2 : rect.width / 2),
          y: rect.y + rect.height / 2,
        }
        assert.ok(rect.width > 0 && rect.height > 0)
        if (geometry === "padding")
          assert.ok(point.x < icon.x, "padding must be outside SVG")
        await page.mouse.move(point.x, point.y)
        await page.mouse.down()
        for (const next of transitions) await mode(next)
        await page.mouse.up()
        return { button: rect, icon, point }
      }
      for (const initial of ["send", "stop"]) {
        const other = initial === "send" ? "stop" : "send"
        for (const geometry of ["padding", "center"]) {
          await check(
            `${initial} stable ${geometry}`,
            initial,
            () => pointer(geometry, []),
            initial === "send" ? 1 : 0,
            initial === "stop" ? 1 : 0,
          )
          await check(
            `${initial} cross-mode ${geometry}`,
            initial,
            () => pointer(geometry, [other]),
            0,
            0,
            true,
          )
          await check(
            `${initial} ABA ${geometry}`,
            initial,
            () => pointer(geometry, [other, initial]),
            0,
            0,
            true,
          )
        }
        await check(
          `${initial} stable Space`,
          initial,
          async () => {
            await button.focus()
            await page.keyboard.press("Space")
          },
          initial === "send" ? 1 : 0,
          initial === "stop" ? 1 : 0,
        )
        await check(
          `${initial} cross-mode Space`,
          initial,
          async () => {
            await button.focus()
            await page.keyboard.down("Space")
            await mode(other)
            await page.keyboard.up("Space")
          },
          0,
          0,
          true,
        )
        await check(
          `${initial} stable Enter`,
          initial,
          async () => {
            await button.focus()
            await page.keyboard.press("Enter")
          },
          initial === "send" ? 1 : 0,
          initial === "stop" ? 1 : 0,
        )
        await check(
          `${initial} textarea Enter`,
          initial,
          async () => {
            await textarea.focus()
            await page.keyboard.press("Enter")
          },
          1,
          0,
        )
      }
      await check(
        "disabled Stop pointer",
        "stop-disabled",
        () => pointer("padding", []),
        0,
        0,
      )
      await check(
        "Stop disabled during pointer",
        "stop",
        () => pointer("padding", ["stop-disabled"]),
        0,
        0,
      )
      await check(
        "disabled Stop textarea Enter queues draft",
        "stop-disabled",
        async () => {
          await textarea.focus()
          await page.keyboard.press("Enter")
        },
        1,
        0,
      )
      assert.deepEqual(errors, [], `${engine}: browser errors`)
      results.push({ engine, version: browser.version(), errors, completed: true })
    } catch (error) {
      failed = true
      results.push({ engine, error: String(error.stack ?? error), completed: false })
    } finally {
      await browser?.close()
    }
  }
  assert.equal(await hash(), before, "component changed during probe")
} catch (error) {
  failed = true
  results.push({ error: String(error.stack ?? error) })
} finally {
  await server?.close()
  await rm(cache, { recursive: true, force: true })
}
console.log(JSON.stringify({ component_sha256: before, ok: !failed, results }, null, 2))
if (failed) process.exitCode = 1
