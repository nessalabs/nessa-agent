import { createServer } from "node:http"
import { randomBytes, timingSafeEqual } from "node:crypto"
import { readFile } from "node:fs/promises"
import { ExperimentFailure } from "./session.mjs"

function readBody(req) {
  return new Promise((resolve, reject) => {
    let size = 0
    const chunks = []
    req.on("data", (chunk) => {
      size += chunk.length
      if (size > 4096) {
        reject(new ExperimentFailure("body_too_large"))
        return
      }
      chunks.push(chunk)
    })
    req.on("end", () => {
      try {
        resolve(JSON.parse(Buffer.concat(chunks).toString()))
      } catch {
        reject(new ExperimentFailure("invalid_json"))
      }
    })
    req.on("error", reject)
  })
}

export async function viewerServer(session, savedDownloads = []) {
  const token = randomBytes(32).toString("hex")
  const assets = new Map(
    await Promise.all(
      [
        ["/", "index.html", "text/html; charset=utf-8"],
        ["/viewer.js", "viewer.js", "text/javascript; charset=utf-8"],
        ["/viewer.css", "viewer.css", "text/css; charset=utf-8"],
      ].map(async ([route, name, type]) => [
        route,
        { type, body: await readFile(new URL(`../viewer/${name}`, import.meta.url)) },
      ]),
    ),
  )
  let origin
  const server = createServer(async (req, res) => {
    res.setHeader("Cache-Control", "no-store")
    res.setHeader("X-Content-Type-Options", "nosniff")
    res.setHeader("Referrer-Policy", "no-referrer")
    res.setHeader(
      "Content-Security-Policy",
      "default-src 'self'; img-src 'self' blob:; frame-ancestors 'self'; object-src 'none'; base-uri 'none'",
    )
    const json = (status, value) => {
      res.writeHead(status, { "Content-Type": "application/json" })
      res.end(JSON.stringify(value))
    }
    try {
      if (req.headers.host !== new URL(origin).host)
        return json(403, { error: "invalid_host" })
      const asset = assets.get(req.url)
      if (req.method === "GET" && asset) {
        res.writeHead(200, { "Content-Type": asset.type })
        return res.end(asset.body)
      }
      const supplied = Buffer.from(req.headers.authorization ?? "")
      const expected = Buffer.from(`Bearer ${token}`)
      if (supplied.length !== expected.length || !timingSafeEqual(supplied, expected))
        return json(401, { error: "unauthorized" })
      if (req.headers.origin && req.headers.origin !== origin)
        return json(403, { error: "invalid_origin" })
      if (req.method === "GET" && req.url === "/status")
        return json(200, { ...session.status(), downloads: savedDownloads })
      if (req.method === "GET" && req.url === "/frame") {
        const frame = await session.run({ type: "frame" })
        res.writeHead(200, { "Content-Type": "image/jpeg" })
        return res.end(frame)
      }
      if (req.method === "POST" && req.url === "/command") {
        if (req.headers["content-type"] !== "application/json")
          return json(415, { error: "json_required" })
        const command = await readBody(req)
        if (command?.type === "frame") throw new ExperimentFailure("invalid_command")
        await session.run(command)
        return json(200, { ok: true })
      }
      return json(404, { error: "not_found" })
    } catch (error) {
      const status =
        error instanceof ExperimentFailure
          ? error.code === "body_too_large"
            ? 413
            : 409
          : 500
      json(status, {
        error: error instanceof ExperimentFailure ? error.code : "internal_error",
      })
    }
  })
  server.requestTimeout = 10000
  server.headersTimeout = 10000
  server.maxConnections = 32
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  origin = `http://127.0.0.1:${server.address().port}`
  return {
    origin,
    token,
    url: `${origin}/#${token}`,
    close: () => new Promise((resolve) => server.close(resolve)),
  }
}
