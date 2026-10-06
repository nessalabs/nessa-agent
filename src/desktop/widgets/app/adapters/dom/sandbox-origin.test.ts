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
  const scheme = {
    url: "nessa-sandbox://localhost/proxy.html",
    origin: "nessa-sandbox://localhost",
  }
  const windows = {
    url: "http://nessa-sandbox.localhost/proxy.html",
    origin: "http://nessa-sandbox.localhost",
  }

  it("is the nessa-sandbox scheme, an http host on Windows (other)", () => {
    expect(sandboxFor("macos", document, true)).toEqual(scheme)
    expect(sandboxFor("linux", document, true)).toEqual(scheme)
    expect(sandboxFor("other", document, true)).toEqual(windows)
  })

  it("uses the scheme when the host registered it, even if the page names a proxy", () => {
    name("http://127.0.0.1:43941/proxy.html")
    expect(sandboxFor("linux", document, true)).toEqual(scheme)
    expect(sandboxFor("macos", document, true)).toEqual(scheme)
    expect(sandboxFor("other", document, true)).toEqual(windows)
  })

  it("uses the proxy the page names when the host registered no scheme", () => {
    name("http://127.0.0.1:43941/proxy.html")
    const named = {
      url: "http://127.0.0.1:43941/proxy.html",
      origin: "http://127.0.0.1:43941",
    }
    expect(sandboxFor("linux", document, false)).toEqual(named)
    expect(sandboxFor("macos", document, false)).toEqual(named)
    expect(sandboxFor("other", document, false)).toEqual(named)
  })
})

describe("the browser build's proxy", () => {
  it("is the one the page names, on another origin than the page's", () => {
    name("http://127.0.0.1:43941/proxy.html")
    expect(document.location.origin).not.toBe("http://127.0.0.1:43941")
    expect(sandboxFor("browser", document, false)).toEqual({
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
