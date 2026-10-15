import assert from "node:assert/strict"
import { once } from "node:events"
import {
  mkdtemp,
  mkdir,
  readFile,
  realpath,
  rm,
  symlink,
  utimes,
  writeFile,
} from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import test from "node:test"

import tailwindcss from "@tailwindcss/vite"
import { build, createServer } from "vite"
import WebSocket from "ws"

// Exercise the real compiler and Vite hotUpdate hooks, without modifying the
// checkout or shared UI dependency. Copy its CSS verbatim so its @source paths
// and the application's import options decide which fixture files are scanned.
for (const entry of ["src/styles.css", "src/desktop/styles.css"]) {
  test(
    `${entry} watches UI sources without reloading for backend edits`,
    { timeout: 30_000 },
    async (context) => {
      const root = await realpath(
        await mkdtemp(join(tmpdir(), "nessa-tailwind-sources-")),
      )
      let server
      let socket
      context.after(async () => {
        socket?.terminate()
        try {
          await server?.close()
        } finally {
          await rm(root, { recursive: true, force: true })
        }
      })
      const put = async (path, content) => {
        const file = join(root, path)
        await mkdir(dirname(file), { recursive: true })
        await writeFile(file, content)
        return file
      }
      await symlink(resolve("node_modules"), join(root, "node_modules"), "junction")
      await put(entry, await readFile(entry, "utf8"))
      for (const name of ["app.css", "styles.css", "theme.css"])
        await put(
          `shared/${name}`,
          await readFile(`node_modules/@nessa-ui/react/src/${name}`, "utf8"),
        )
      const frontend = await put(`${dirname(entry)}/probe.tsx`, '"z-[8101]"')
      const component = await put("shared/components/probe.tsx", '"z-[8102]"')
      const composite = await put("shared/composites/probe.tsx", '"z-[8103]"')
      const sharedFrontend = await put(
        "src/conversation/ui/published-files.tsx",
        '"z-[8104]"',
      )
      const backend = await put(
        "crates/server/src/probe.rs",
        'const CLASS: &str = "z-[8199]";',
      )

      const sources = {
        alias: { "@nessa-ui/react/app.css": join(root, "shared/app.css") },
      }
      let watching
      server = await createServer({
        root,
        configFile: false,
        cacheDir: join(root, ".cache"),
        logLevel: "error",
        plugins: [
          tailwindcss(),
          {
            name: "watch-event-fence",
            enforce: "post",
            configureServer(starting) {
              watching = once(starting.watcher, "ready")
            },
            hotUpdate({ file }) {
              // Tailwind sends its direct full-reload inside hotUpdate, before
              // this fence on the same socket. Ordinary Vite updates may also
              // arrive; this fence does not attribute or exclude those (#937).
              this.environment.hot.send({
                type: "custom",
                event: "watch-event",
                data: file,
              })
            },
          },
        ],
        resolve: sources,
        server: { host: "127.0.0.1", port: 0 },
      })
      await server.listen()
      await watching
      const url = server.resolvedUrls.local[0]
      const packets = []
      socket = new WebSocket(
        `${url.replace("http:", "ws:")}?token=${encodeURIComponent(server.config.webSocketToken)}`,
        "vite-hmr",
        {
          headers: { Origin: url },
        },
      )
      socket.on("message", (data) => packets.push(JSON.parse(String(data))))
      while (!packets.some((packet) => packet.type === "connected"))
        await once(socket, "message")

      const css = async () => {
        const response = await fetch(new URL(`${entry}?import`, url), {
          headers: { "Sec-Fetch-Dest": "script" },
        })
        assert.equal(response.status, 200)
        return response.text()
      }
      const initial = await css()
      for (const value of [8101, 8102, 8103, 8104])
        assert.ok(initial.includes(`z-index: ${value};`), `registered UI source ${value}`)

      const changed = async (file, effect) => {
        packets.length = 0
        await effect()
        while (
          !packets.some(
            (packet) =>
              packet.type === "custom" &&
              packet.event === "watch-event" &&
              packet.data === file,
          )
        )
          await once(socket, "message")
      }
      const before = await readFile(backend, "utf8")
      await changed(backend, async () => {
        // A normal CSS update preserves the document. Exercise it beside the
        // backend event so the reload assertion cannot reject all HMR packets.
        const client = server.environments.client
        const modules = client.moduleGraph.getModulesByFile(join(root, entry))
        assert.ok(modules?.size, "loaded stylesheet module")
        for (const module of modules) await client.reloadModule(module)
        await utimes(backend, new Date(), new Date())
      })
      assert.equal(await readFile(backend, "utf8"), before)
      assert.deepEqual(
        packets.filter((packet) => packet.type === "full-reload"),
        [],
        "backend touch must not interrupt the frontend document",
      )
      assert.ok(
        packets.some((packet) => packet.type === "update"),
        "ordinary CSS update is compatible with document lifetime",
      )
      assert.ok(
        !initial.includes("z-index: 8199;"),
        "backend classes are outside the source boundary",
      )
      assert.equal(
        server.environments.client.moduleGraph.getModulesByFile(backend)?.size ?? 0,
        0,
        "backend files stay outside the CSS dependency graph",
      )

      for (const [file, value] of [
        [frontend, 8201],
        [component, 8202],
        [composite, 8203],
        [sharedFrontend, 8204],
      ]) {
        await changed(file, () => writeFile(file, `"z-[${value}]"`))
        while (!packets.some((packet) => ["full-reload", "update"].includes(packet.type)))
          await once(socket, "message")
        assert.ok(
          packets.some((packet) => packet.type === "update"),
          "registered UI edit must publish a CSS HMR update",
        )
        assert.ok(
          (await css()).includes(`z-index: ${value};`),
          `changed UI class ${value} is compiled`,
        )
      }
      const production = await build({
        root,
        configFile: false,
        logLevel: "error",
        plugins: [tailwindcss()],
        resolve: sources,
        build: {
          write: false,
          cssMinify: false,
          rollupOptions: { input: join(root, entry) },
        },
      })
      const compiled = production.output
        .filter((file) => file.type === "asset" && file.fileName.endsWith(".css"))
        .map((file) => String(file.source))
        .join("\n")
      for (const value of [8201, 8202, 8203, 8204])
        assert.ok(compiled.includes(`z-index: ${value};`), `production UI class ${value}`)
      assert.ok(
        !compiled.includes("z-index: 8199;"),
        "production excludes backend classes",
      )
    },
  )
}
