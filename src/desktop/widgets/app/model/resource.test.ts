/** An app's UI resource, from `resources/read` (#349, L1–L2), and what a host draws for a view. */
import { describe, expect, it } from "vitest"
import { appDraws, appLines, firstView, type AppViewState } from "./app-view"
import { appliedCsp } from "./csp"
import type { FailedReason, Lifecycle } from "./lifecycle"
import { appMimeType, uiResource } from "./resource"

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
    })
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

describe("what a host draws for an app view", () => {
  const initialize = { protocolVersion: "2026-01-26", appName: "App" }
  const lifecycleOf = (kind: Lifecycle["kind"], failed: FailedReason): Lifecycle => {
    switch (kind) {
      case "initializing":
      case "live":
      case "ending":
        return { kind, initialize }
      case "failed":
        return { kind, reason: failed }
      default:
        return { kind }
    }
  }
  const view = ({
    lifecycle,
    failed = "load",
    ...fields
  }: Partial<Omit<AppViewState, "lifecycle">> & {
    lifecycle: Lifecycle["kind"]
    failed?: FailedReason
  }): AppViewState => ({
    ...firstView,
    ...fields,
    lifecycle: lifecycleOf(lifecycle, failed),
  })

  it("the placeholder over a hidden frame while it loads, then the frame", () => {
    expect(appDraws("pane", view({ lifecycle: "reading" }))).toEqual({
      frame: "none",
      waiting: true,
      notices: [],
    })
    for (const lifecycle of ["proxy", "loading", "initializing"] as const)
      expect(appDraws("inline", view({ lifecycle, frame: true }))).toEqual({
        frame: "hidden",
        waiting: true,
        notices: [],
      })
    expect(appDraws("pane", view({ lifecycle: "live", frame: true }))).toEqual({
      frame: "shown",
      waiting: false,
      notices: [],
    })
  })

  it("one line for a view that failed, with close where the place has one", () => {
    expect(appDraws("inline", view({ lifecycle: "failed", failed: "load" }))).toEqual({
      frame: "none",
      waiting: false,
      line: { text: appLines.load, closes: false },
      notices: [],
    })
    expect(
      appDraws("window", view({ lifecycle: "failed", failed: "server-gone" })).line,
    ).toEqual({
      text: appLines.serverGone,
      closes: true,
    })
  })

  it("notices above a live app: its server gone, and what its CSP blocked", () => {
    expect(
      appDraws(
        "pane",
        view({
          lifecycle: "live",
          frame: true,
          serverGone: true,
          anyBlocked: true,
          blocked: ["https://a.example", "https://b.example"],
        }),
      ).notices,
    ).toEqual([
      appLines.serverGone,
      `${appLines.blocked}: https://a.example, https://b.example`,
    ])
    expect(
      appDraws("pane", view({ lifecycle: "live", frame: true, anyBlocked: true }))
        .notices,
    ).toEqual([appLines.blocked])
  })
})
