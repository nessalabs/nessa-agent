/** The policy an app is loaded under, built only from its `_meta.ui.csp` (#349, Sandbox). */
import { describe, expect, it } from "vitest"
import { sandboxMethods } from "./sandbox-methods"
import {
  appDocument,
  appliedCsp,
  approvedDomains,
  cspPolicy,
  domainsPerList,
  parseSource,
} from "./csp"

const policyOf = (csp: unknown) =>
  Object.fromEntries(
    cspPolicy(appliedCsp({ csp } as never))
      .split("; ")
      .map((directive) => {
        const [name, ...sources] = directive.split(" ")
        return [name, sources]
      }),
  )

describe("with nothing declared", () => {
  it("is the spec's restrictive default, with no network at all", () => {
    expect(policyOf(undefined)).toEqual({
      "default-src": ["'none'"],
      "script-src": ["'self'", "'unsafe-inline'"],
      "style-src": ["'self'", "'unsafe-inline'"],
      "connect-src": ["'none'"],
      "img-src": ["'self'", "data:"],
      "font-src": ["'self'"],
      "media-src": ["'self'", "data:"],
      "frame-src": ["'none'"],
      "object-src": ["'none'"],
      "base-uri": ["'self'"],
      "form-action": ["'none'"],
    })
    expect(appliedCsp(undefined)).toEqual(appliedCsp({}))
    expect(appliedCsp("csp")).toEqual(appliedCsp({}))
  })
})

describe("with domains declared", () => {
  it("names each where the spec maps it, spelled from its parts", () => {
    const policy = policyOf({
      connectDomains: ["https://api.openweathermap.org", "wss://realtime.service.com"],
      resourceDomains: ["https://cdn.jsdelivr.net", "https://*.cloudflare.com"],
      frameDomains: ["https://www.youtube.com"],
      baseUriDomains: ["https://cdn.example.com/"],
    })
    expect(policy["connect-src"]).toEqual([
      "https://api.openweathermap.org",
      "wss://realtime.service.com",
    ])
    for (const directive of [
      "script-src",
      "style-src",
      "img-src",
      "font-src",
      "media-src",
    ])
      expect(policy[directive], directive).toEqual(
        expect.arrayContaining(["https://cdn.jsdelivr.net", "https://*.cloudflare.com"]),
      )
    // Never a declared frame: the app's frame loads nothing but its document.
    expect(policy["frame-src"]).toEqual(["'none'"])
    expect(policy["base-uri"]).toEqual(["https://cdn.example.com"])
    expect(policy["default-src"]).toEqual(["'none'"])
  })

  it("leaves out anything that is not a scheme, a host and a port", () => {
    for (const declared of [
      "*",
      "https:",
      "'unsafe-eval'",
      "'self'",
      "data:",
      "blob:",
      "https://a.example/path",
      "https://a.example; script-src *",
      "https://a.example 'unsafe-eval'",
      "https://a.example,https://b.example",
      "https://*",
      "https://*.*.example.com",
      "https://a*.example.com",
      "ftp://a.example",
      "javascript://a.example",
      "https://a.example:0",
      "https://a.example:65536",
      "https://-a.example",
      "https://a.example\n",
      "https://user@a.example",
      `https://${"a".repeat(250)}.com`,
      7,
      null,
    ])
      expect(parseSource(declared as never), String(declared)).toBeUndefined()
    expect(parseSource("HTTPS://CDN.Example.com:8443")).toEqual({
      scheme: "https",
      host: "cdn.example.com",
      port: 8443,
    })
    expect(parseSource("http://localhost:3000")).toEqual({
      scheme: "http",
      host: "localhost",
      port: 3000,
    })
    expect(parseSource("ws://127.0.0.1:9000")).toEqual({
      scheme: "ws",
      host: "127.0.0.1",
      port: 9000,
    })
  })

  it("never lets a declared value loosen the policy: a keyword in a list is dropped, not applied", () => {
    const policy = cspPolicy(
      appliedCsp({
        csp: {
          connectDomains: ["*", "https://ok.example"],
          resourceDomains: ["'unsafe-eval'", "https://a.example; connect-src *"],
        },
      }),
    )
    expect(policy).not.toContain("unsafe-eval")
    expect(policy).toContain("connect-src https://ok.example;")
    expect(policy.split("; ").filter((d) => d.startsWith("connect-src"))).toHaveLength(1)
  })

  it("applies at most so many per list, once each, and says what it applied", () => {
    const many = Array.from(
      { length: domainsPerList + 5 },
      (_, i) => `https://h${i}.example`,
    )
    const applied = appliedCsp({
      csp: { connectDomains: [...many, "https://h0.example"], frameDomains: ["nope"] },
    })
    expect(applied.connect).toHaveLength(domainsPerList)
    expect(approvedDomains(applied)).toEqual({
      connectDomains: many.slice(0, domainsPerList),
    })
    expect(approvedDomains(appliedCsp(undefined))).toEqual({})
  })
})

describe("the app's document", () => {
  it("is the policy first, then the reporter, then the app — in standards mode", () => {
    const html = "<!DOCTYPE html><html><head><script>app()</script></head></html>"
    const document = appDocument(html, appliedCsp(undefined))
    expect(
      document.startsWith(
        '<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src \'none\'; ',
      ),
    ).toBe(true)
    const policyAt = document.indexOf("Content-Security-Policy")
    const reporterAt = document.indexOf("securitypolicyviolation")
    const appAt = document.indexOf("app()")
    expect(policyAt).toBeLessThan(reporterAt)
    expect(reporterAt).toBeLessThan(appAt)
    expect(document.endsWith(html)).toBe(true)
  })

  it("holds a policy that cannot close its attribute", () => {
    const document = appDocument(
      "",
      appliedCsp({
        csp: { connectDomains: ['https://a.example"><script>x()</script>'] },
      }),
    )
    const meta = document.slice(0, document.indexOf("<script>"))
    expect(meta.match(/"/g)).toHaveLength(4)
    expect(meta).not.toContain("x()")
  })
})

describe("frames", () => {
  it("are never applied nor approved, whatever is declared", () => {
    const applied = appliedCsp({ csp: { frameDomains: ["https://www.youtube.com"] } })
    expect(cspPolicy(applied)).toContain("frame-src 'none';")
    expect(approvedDomains(applied)).toEqual({})
  })
})

describe("the reporter", () => {
  it("says what is blocked and that the document is going, both registered before the app's markup", () => {
    const document = appDocument("<script>app()</script>", appliedCsp(undefined))
    const reporterAt = document.indexOf("<script>")
    for (const said of [
      'addEventListener("securitypolicyviolation"',
      sandboxMethods.cspViolation,
      'addEventListener("pagehide"',
      sandboxMethods.appLeft,
    ])
      expect(document.indexOf(said), said).toBeGreaterThan(reporterAt)
    for (const said of ['addEventListener("pagehide"', sandboxMethods.appLeft])
      expect(document.indexOf(said), said).toBeLessThan(document.indexOf("app()"))
  })
})
