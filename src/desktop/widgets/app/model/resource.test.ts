/** An app's UI resource, from `resources/read` (#349, L1–L2). */
import { describe, expect, it } from "vitest"
import { appliedCsp } from "./csp"
import { appMimeType, frameAllow, uiResource } from "./resource"

const uri = "ui://weather-server/dashboard-template"
const item = (fields: Record<string, unknown>) => ({
  contents: [{ uri, mimeType: appMimeType, ...fields }],
})

describe("the UI resource", () => {
  it("is the spec's example: text, and the CSP its _meta.ui declares", () => {
    const resource = uiResource(
      {
        contents: [
          {
            uri,
            mimeType: appMimeType,
            text: "<!DOCTYPE html><html>...</html>",
            _meta: {
              ui: {
                csp: {
                  connectDomains: ["https://api.openweathermap.org"],
                  resourceDomains: ["https://cdn.jsdelivr.net"],
                },
                prefersBorder: true,
              },
            },
          },
        ],
      },
      uri,
    )
    expect(resource).toEqual({
      html: "<!DOCTYPE html><html>...</html>",
      csp: appliedCsp({
        csp: {
          connectDomains: ["https://api.openweathermap.org"],
          resourceDomains: ["https://cdn.jsdelivr.net"],
        },
      }),
      permissions: { microphone: false },
    })
  })

  it("grants the microphone only to an app that asks for it, as a flag or an object", () => {
    const asking = (microphone: unknown) =>
      uiResource(item({ text: "<p></p>", _meta: { ui: { permissions: { microphone } } } }), uri)
        ?.permissions.microphone
    expect(asking({})).toBe(true)
    expect(asking(true)).toBe(true)
    expect(asking(false)).toBe(false)
    expect(asking("yes")).toBe(false)
    expect(uiResource(item({ text: "<p></p>" }), uri)?.permissions.microphone).toBe(false)
    expect(frameAllow({ microphone: true })).toBe("microphone *")
    expect(frameAllow({ microphone: false })).toBe("")
  })

  it("decodes a base64 blob as UTF-8", () => {
    const blob = btoa(String.fromCharCode(...new TextEncoder().encode("<p>é</p>")))
    expect(uiResource(item({ blob }), uri)?.html).toBe("<p>é</p>")
  })

  it("is none for another type, another URI, no content, both, or bytes that are not UTF-8", () => {
    for (const result of [
      { contents: [{ uri, mimeType: "text/html", text: "<p>" }] },
      { contents: [{ uri: "ui://other", mimeType: appMimeType, text: "<p>" }] },
      item({}),
      item({ text: "  " }),
      item({ text: "<p>", blob: "PHA+" }),
      item({ blob: "not base64!" }),
      item({ blob: btoa("\xff\xfe") }),
      { contents: "x" },
      {},
    ])
      expect(uiResource(result as never, uri), JSON.stringify(result)).toBeUndefined()
  })
})
