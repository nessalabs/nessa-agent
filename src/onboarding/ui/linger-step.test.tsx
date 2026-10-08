import * as React from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"

import type { LingerView } from "../model/linger"
import { LingerStep } from "./linger-step"

function markup(view: LingerView | undefined, pending = false) {
  return renderToStaticMarkup(
    React.createElement(LingerStep, {
      view,
      pending,
      onAccept: () => {},
      onDecline: () => {},
      onFinish: () => {},
    }),
  )
}

const views: LingerView[] = [
  { shown: "offer", audit: "not-required" },
  { shown: "declined", audit: "recorded" },
  { shown: "refused", audit: "recorded" },
  { shown: "waiting", audit: "not-required" },
  { shown: "unsupported", audit: "not-required" },
  { shown: "unconfirmed", audit: "not-required" },
  { shown: "not-applicable", audit: "not-required" },
]

describe("the linger step", () => {
  it("claims logged-out operation only when the host says enabled", () => {
    const enabled = markup({ shown: "enabled", audit: "recorded" })
    expect(enabled).toContain("keeps running")
    expect(enabled).toContain('data-linger-claim="yes"')
    expect(enabled).toContain("Start using Nessa")
    expect(enabled).not.toContain("Keep it running")
    for (const view of views) {
      const html = markup(view)
      expect(html).not.toContain("keeps running")
      expect(html).toContain('data-linger-claim="no"')
    }
  })

  it("offers the choice, a retry, and nothing while the prompt is open", () => {
    const offer = markup({ shown: "offer", audit: "not-required" })
    expect(offer).toContain("Keep it running")
    expect(offer).toContain("Only while I’m signed in")
    expect(offer).not.toContain("Start using Nessa")
    const refused = markup({ shown: "refused", audit: "failed" })
    expect(refused).toContain("Try again")
    expect(refused).toContain("Start using Nessa")
    expect(refused).toContain("Nessa could not record that decision.")
    const waiting = markup({ shown: "offer", audit: "not-required" }, true)
    expect(waiting).toContain('data-linger="waiting"')
    expect(waiting).not.toContain("Keep it running")
    expect(waiting).not.toContain("Start using Nessa")
  })
})
