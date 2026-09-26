import * as React from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it, vi } from "vitest"
import { startupNotice } from "./startup-notice"

const markup = (node: React.ReactNode) =>
  node === null
    ? ""
    : renderToStaticMarkup(React.createElement(React.Fragment, null, node))

describe("the panel's startup notice (ADR 221)", () => {
  it("says the step while the host is working, with nothing to press", () => {
    const shown = markup(
      startupNotice({ revision: 2, state: "starting", step: "replacing" }, vi.fn()),
    )
    expect(shown).toContain("Finishing the last update…")
    expect(shown).not.toContain("Try again")
  })

  it("says Nessa could not start, with Try again, once the host has failed", () => {
    const shown = markup(
      startupNotice(
        {
          revision: 3,
          state: "failed",
          message: "Gateway retirement was not acknowledged",
        },
        vi.fn(),
      ),
    )
    expect(shown).toContain("Nessa couldn’t start.")
    expect(shown).toContain("Try again")
    expect(shown).not.toContain("retirement")
    expect(shown.toLowerCase()).not.toContain("gateway")
  })

  it("gives the slot back once there is nothing to say", () => {
    expect(startupNotice(undefined, vi.fn())).toBeNull()
    expect(startupNotice({ revision: 4, state: "ready" }, vi.fn())).toBeNull()
    expect(startupNotice({ revision: 0, state: "unmanaged" }, vi.fn())).toBeNull()
    // Not being able to ask the host is not a startup failure to announce.
    expect(
      startupNotice({ state: "unavailable", message: "no host" }, vi.fn()),
    ).toBeNull()
  })
})
