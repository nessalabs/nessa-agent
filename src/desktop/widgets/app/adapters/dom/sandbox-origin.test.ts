// @vitest-environment jsdom
/** Where this window's sandbox proxy is, by host — and when it has none (#349, Sandbox). */
import { afterEach, describe, expect, it } from "vitest"
import { pageSandbox, platformFor, sandboxFor, sandboxMetaName } from "./sandbox-origin"

const name = (url: string | null) => {
  document.head.querySelector(`meta[name="${sandboxMetaName}"]`)?.remove()
  if (url === null) return
  const meta = document.createElement("meta")
  meta.name = sandboxMetaName
  meta.content = url
  document.head.append(meta)
}

afterEach(() => name(null))

describe("the desktop app's proxy", () => {
  it("is the nessa-sandbox scheme, an http host on Windows (other)", () => {
    expect(sandboxFor("macos", document)).toEqual({
      url: "nessa-sandbox://localhost/proxy.html",
      origin: "nessa-sandbox://localhost",
    })
    expect(sandboxFor("linux", document)).toEqual(sandboxFor("macos", document))
    expect(sandboxFor("other", document)).toEqual({
      url: "http://nessa-sandbox.localhost/proxy.html",
      origin: "http://nessa-sandbox.localhost",
    })
  })
})

describe("the browser build's proxy", () => {
  it("is the one the page names, on another origin than the page's", () => {
    name("http://127.0.0.1:43941/proxy.html")
    expect(document.location.origin).not.toBe("http://127.0.0.1:43941")
    expect(sandboxFor("browser", document)).toEqual({
      url: "http://127.0.0.1:43941/proxy.html",
      origin: "http://127.0.0.1:43941",
    })
  })

  it("is none when the page names none, nothing it can parse, not http(s), or its own origin", () => {
    for (const url of [
      null,
      "",
      "not a url",
      "javascript:alert(1)",
      "nessa-sandbox://localhost/proxy.html",
      `${document.location.origin}/proxy.html`,
    ]) {
      name(url)
      expect(pageSandbox(document), String(url)).toBeUndefined()
    }
  })
})

describe("the platform an app is told", () => {
  it("is web in a browser, desktop in the app", () => {
    expect(platformFor("browser")).toBe("web")
    for (const host of ["macos", "linux", "other"] as const)
      expect(platformFor(host)).toBe("desktop")
  })
})
