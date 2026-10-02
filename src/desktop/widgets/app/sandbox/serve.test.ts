/**
 * The browser build's sandbox listener (#349, Sandbox): it serves the proxy
 * and nothing else, and the page it names the proxy in can find it.
 */
import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"
import { departureTokenSlot, sandboxMethods } from "../model/sandbox-methods"
import { sandboxMetaName } from "../adapters/dom/sandbox-origin"
import { sandboxResponse, withMeta } from "./serve"

const proxy = readFileSync(new URL("./proxy.html", import.meta.url))

describe("the listener", () => {
  it("serves the proxy at GET /proxy.html, never cached", () => {
    const answer = sandboxResponse("GET", "/proxy.html?x=1", proxy)
    expect(answer.status).toBe(200)
    expect(answer.headers["Content-Type"]).toBe("text/html; charset=utf-8")
    expect(answer.headers["Cache-Control"]).toBe("no-store")
    expect(answer.body).toBe(proxy)
  })

  it("serves nothing else", () => {
    for (const [method, url] of [
      ["GET", "/"],
      ["GET", "/desktop.html"],
      ["GET", "/proxy.html/"],
      ["GET", "//["],
      ["GET", "/%2e%2e/proxy.html"],
      ["GET", "/src/desktop/widgets/app/sandbox/proxy.html"],
      ["POST", "/proxy.html"],
      [undefined, "/proxy.html"],
    ] as const)
      expect(sandboxResponse(method, url, proxy).status, `${method} ${url}`).toBe(404)
  })

  it("names the proxy in a page's head, where composition reads it", () => {
    expect(
      withMeta(
        "<html><head><title>x</title></head></html>",
        "http://127.0.0.1:1/proxy.html",
      ),
    ).toBe(
      `<html><head><title>x</title><meta name="${sandboxMetaName}" content="http://127.0.0.1:1/proxy.html"></head></html>`,
    )
  })
})

describe("the proxy", () => {
  // The proxy's script is plain JavaScript, served as it is written, so the
  // methods it speaks are checked against the host's own here.
  const text = proxy.toString("utf8")

  it("speaks the methods the host listens for", () => {
    for (const method of [...Object.values(sandboxMethods), departureTokenSlot])
      expect(text, method).toContain(`"${method}"`)
  })

  it("frames the app with allow-scripts alone, and applies the host's policy first", () => {
    expect(text).toContain('app.setAttribute("sandbox", "allow-scripts")')
    expect(text.indexOf('meta.setAttribute("content", policy)')).toBeLessThan(
      text.indexOf("app = document.createElement"),
    )
  })
})
