/** What a host draws for an app view (#349, What a host draws — app rows). */
import { describe, expect, it } from "vitest"
import { appDraws, appLines, firstView, type AppViewState } from "./app-view"
import type { FailedReason, Lifecycle } from "./lifecycle"

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
      expect(appDraws("inline", view({ lifecycle }))).toEqual({
        frame: "hidden",
        waiting: true,
        notices: [],
      })
    expect(appDraws("pane", view({ lifecycle: "live" }))).toEqual({
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
      appDraws("pane", view({ lifecycle: "live", anyBlocked: true })).notices,
    ).toEqual([appLines.blocked])
  })
})
