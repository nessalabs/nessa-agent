#!/usr/bin/env node
/**
 * A stand-in for cloudflared, for the conversation stress check's `tunnel`
 * step: the gateway starts it as it would the real program
 * (`NESSA_TUNNEL_PROGRAM`, debug builds only), with the same arguments, and
 * it answers the same two questions on its metrics address:
 *
 *   GET /ready         200 once it serves
 *   GET /quicktunnel   {"hostname": "http://127.0.0.1:<port>"}, the public
 *                      side, an origin rather than a bare hostname, which
 *                      the gateway accepts only on loopback
 *
 * The public side copies each TCP connection to the ingress `--url` names,
 * so the WebSocket a dialer opens reaches the gateway's ingress as it would
 * through Cloudflare's edge. Each one is logged as `connection N`; its
 * output is the gateway's `<tunnel>/cloudflared.log`, which the step reads.
 * It runs until a signal ends it, as cloudflared does.
 */
import { createServer as createHttp } from "node:http"
import { connect, createServer } from "node:net"

function argument(name) {
  const at = process.argv.indexOf(name)
  return at === -1 ? undefined : process.argv[at + 1]
}

function hostPort(value, what) {
  const match = /^(?:https?:\/\/)?([^:/]+):(\d+)\/?$/.exec(value ?? "")
  if (!match) {
    console.error(`fake cloudflared: no ${what} in ${JSON.stringify(process.argv)}`)
    process.exit(2)
  }
  return { host: match[1], port: Number(match[2]) }
}

const metrics = hostPort(argument("--metrics"), "--metrics")
// A named tunnel is routed in Cloudflare's dashboard, not by an argument:
// this one serves only the quick form.
const ingress = hostPort(argument("--url"), "--url")

let connections = 0
const edge = createServer((socket) => {
  connections += 1
  console.log(`connection ${connections}`)
  const upstream = connect(ingress.port, ingress.host)
  socket.pipe(upstream).pipe(socket)
  const end = () => {
    socket.destroy()
    upstream.destroy()
  }
  socket.on("error", end)
  upstream.on("error", end)
  socket.on("close", end)
  upstream.on("close", end)
})

edge.listen(0, "127.0.0.1", () => {
  const { port } = edge.address()
  const hostname = `http://127.0.0.1:${port}`
  createHttp((request, response) => {
    if (request.url === "/ready") {
      response.writeHead(200, { "content-type": "application/json" })
      response.end(JSON.stringify({ status: 200, readyConnections: 1 }))
      return
    }
    if (request.url === "/quicktunnel") {
      response.writeHead(200, { "content-type": "application/json" })
      response.end(JSON.stringify({ hostname }))
      return
    }
    response.writeHead(404)
    response.end()
  }).listen(metrics.port, metrics.host, () => {
    console.log(
      `fake cloudflared ${process.pid} at ${hostname} for ${ingress.host}:${ingress.port}`,
    )
  })
})

for (const signal of ["SIGTERM", "SIGINT"]) process.on(signal, () => process.exit(0))
