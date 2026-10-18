/** Failure-only observations for gateway window opening; never a readiness rule. */
import { mkdir, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { randomUUID } from "node:crypto"

/** Query, fragment, credentials and arbitrary path segments are never diagnostic data. */
export function diagnosticUrl(value) {
  try {
    const url = new URL(value)
    if (!["http:", "https:", "ws:", "wss:"].includes(url.protocol)) return "[unavailable]"
    const path = [
      "/browser",
      "/browser/login",
      "/mcp-resources",
      "/desktop.html",
    ].includes(url.pathname)
      ? url.pathname
      : "/[redacted]"
    const host = ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname)
      ? url.hostname
      : "[remote]"
    return `${url.protocol}//${host}${url.port ? `:${url.port}` : ""}${path}`
  } catch {
    return "[unavailable]"
  }
}

/** Tracks only unfinished work, without retaining headers, bodies or RPC parameters. */
export function watchOpenRequests(page, limit = 256) {
  // The opening diagnostic names only this check's expected methods; arbitrary
  // peer-provided names never become an output channel for frame contents.
  const methods = new Set([
    "conversation.list",
    "conversation.read",
    "conversation.observe",
    "conversation.subscribe",
    "conversation.subscribeList",
    "agents.list",
    "agents.subscribe",
    "mcp.callTool",
    "mcp.readResource",
    "mcp.releaseApp",
    "mcp.sendMessage",
    "mcp.updateModelContext",
  ])
  const types = new Set([
    "document",
    "stylesheet",
    "image",
    "media",
    "font",
    "script",
    "texttrack",
    "xhr",
    "fetch",
    "eventsource",
    "websocket",
    "manifest",
    "other",
  ])
  const http = new Map()
  const sockets = new Map()
  const listeners = []
  let overflow = 0
  let unsupportedFrames = 0
  const listen = (owner, event, handler) => {
    owner.on(event, handler)
    listeners.push(() => owner.off(event, handler))
  }
  listen(page, "request", (request) => {
    if (http.size >= limit) {
      overflow++
      return
    }
    http.set(request, {
      method: ["GET", "POST", "OPTIONS", "PUT", "DELETE", "PATCH", "HEAD"].includes(
        request.method(),
      )
        ? request.method()
        : "other",
      url: diagnosticUrl(request.url()),
      type: types.has(request.resourceType()) ? request.resourceType() : "other",
      startedAt: Date.now(),
    })
  })
  for (const event of ["requestfinished", "requestfailed"])
    listen(page, event, (request) => http.delete(request))
  listen(page, "websocket", (socket) => {
    if (sockets.size >= limit) {
      overflow++
      return
    }
    const state = { url: diagnosticUrl(socket.url()), closed: false, pending: new Map() }
    sockets.set(socket, state)
    const frame = (event, sent) => {
      const payload = event.payload
      if (typeof payload !== "string" || payload.length > 65536) {
        unsupportedFrames++
        return
      }
      let value
      try {
        value = JSON.parse(payload)
      } catch {
        unsupportedFrames++
        return
      }
      if (
        !value ||
        typeof value !== "object" ||
        !Object.hasOwn(value, "type") ||
        !Object.hasOwn(value, "id")
      )
        return
      // IDs are protocol counters, not arbitrary strings copied from a peer.
      if (typeof value.id !== "string" || !/^[0-9]{1,12}$/.test(value.id)) {
        unsupportedFrames++
        return
      }
      const id = value.id
      if (!sent && value.type === "res") {
        state.pending.delete(id)
        return
      }
      if (!sent || value.type !== "req") return
      if (state.pending.size >= limit) {
        overflow++
        return
      }
      const method =
        Object.hasOwn(value, "method") &&
        typeof value.method === "string" &&
        methods.has(value.method)
          ? value.method
          : "[unrecognized]"
      state.pending.set(id, { id, method, startedAt: Date.now() })
    }
    listen(socket, "framesent", (event) => frame(event, true))
    listen(socket, "framereceived", (event) => frame(event, false))
    listen(socket, "close", () => {
      state.closed = true
    })
  })
  return {
    snapshot() {
      return {
        http: [...http.values()],
        rpc: [...sockets.values()].map((state) => ({
          url: state.url,
          closed: state.closed,
          pending: [...state.pending.values()],
        })),
        overflow,
        unsupportedFrames,
        complete: overflow === 0 && unsupportedFrames === 0,
      }
    },
    dispose() {
      for (const remove of listeners.splice(0)) remove()
    },
  }
}

/** Passed to the page: structural state only, sampled immediately after failure. */
export function openingSnapshot() {
  const finite = (value) =>
    typeof value === "number"
      ? Number.isFinite(value)
        ? value
        : String(value)
      : value === "auto"
        ? "auto"
        : null
  const tags = new Set([
    "HTML",
    "BODY",
    "DIV",
    "SPAN",
    "BUTTON",
    "SECTION",
    "ARTICLE",
    "ASIDE",
    "HEADER",
    "FOOTER",
    "MAIN",
    "NAV",
    "DIALOG",
    "SVG",
    "PATH",
  ])
  const animations = document.getAnimations()
  return {
    capturedAt: Date.now(),
    visibility: document.visibilityState,
    readyState: document.readyState,
    fonts: document.fonts?.status ?? null,
    animationCount: animations.length,
    animations: animations.slice(0, 64).map((animation) => {
      const timing = animation.effect?.getTiming?.()
      return {
        playState: animation.playState,
        pending: animation.pending,
        currentTime: finite(animation.currentTime),
        targetTag: tags.has(animation.effect?.target?.tagName)
          ? animation.effect.target.tagName
          : "other",
        timing: timing
          ? {
              delay: finite(timing.delay),
              duration: finite(timing.duration),
              iterations: finite(timing.iterations),
              endDelay: finite(timing.endDelay),
            }
          : null,
      }
    }),
    appStates: [...document.querySelectorAll("[data-app-view]")]
      .slice(0, 64)
      .map((element) => {
        const state = element.getAttribute("data-app-view")
        return ["live", "loading", "unloadable", "blocked", "starting"].includes(state)
          ? state
          : "other"
      }),
  }
}

/** Post-failure sample, explicitly not a reconstruction of the original RAF polling. */
export function sampleFailureRaf() {
  return new Promise((resolve) => {
    const stamps = []
    let done = false
    const finish = () => {
      if (!done) {
        done = true
        resolve({ postFailure: true, stamps })
      }
    }
    const timeout = setTimeout(finish, 200)
    const frame = (stamp) => {
      if (done) return
      stamps.push(stamp)
      if (stamps.length === 2) {
        clearTimeout(timeout)
        finish()
      } else requestAnimationFrame(frame)
    }
    requestAnimationFrame(frame)
  })
}

/** Bounded best-effort capture. Its own faults never replace the opening failure. */
export async function captureOpenFailure(page, tracker, options, progress) {
  const deadline = Date.now() + 1500
  const bounded = async (operation) => {
    let timeout
    try {
      return await Promise.race([
        Promise.resolve()
          .then(operation)
          .then(
            (value) => ({ value }),
            () => ({ unavailable: true }),
          ),
        new Promise((resolve) => {
          timeout = setTimeout(
            () => resolve({ unavailable: true }),
            Math.max(0, deadline - Date.now()),
          )
        }),
      ])
    } finally {
      clearTimeout(timeout)
    }
  }
  try {
    const root =
      options.evidence ??
      (options.out ? dirname(options.out) : (options.shots ?? tmpdir()))
    const directory = join(root, `open-failure-${randomUUID()}`)
    await mkdir(directory, { recursive: true })
    const requests = tracker.snapshot()
    const snapshot = await bounded(() => page.evaluate(openingSnapshot))
    const raf = await bounded(() => page.evaluate(sampleFailureRaf))
    const screenshot = await bounded(() =>
      page.screenshot({
        path: join(directory, "window.png"),
        timeout: Math.max(1, deadline - Date.now()),
      }),
    )
    await writeFile(
      join(directory, "opening.json"),
      `${JSON.stringify({ progress, requests, snapshot, raf, screenshot: screenshot.unavailable ? "unavailable" : "window.png" }, null, 2)}\n`,
    )
    return directory
  } catch {
    return null
  }
}
