import * as React from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"

import type { LingerView } from "../model/linger"
import { LingerStep } from "./onboarding"

function markup(view: LingerView, pending = false) {
  return renderToStaticMarkup(
    React.createElement(LingerStep, {
      view,
      pending,
      onAccept: () => undefined,
      onFinish: () => undefined,
    }),
  )
}

const quiet: LingerView[] = [
  { shown: "offer" },
  { shown: "refused" },
  { shown: "failed" },
  { shown: "unsupported" },
  { shown: "not-applicable" },
]

describe("the linger step", () => {
  it("claims logged-out operation only when the host says enabled", () => {
    const enabled = markup({ shown: "enabled" })
    expect(enabled).toContain("keeps running")
    expect(enabled).toContain('data-linger-claim="yes"')
    for (const view of quiet) {
      const html = markup(view)
      expect(html).not.toContain("keeps running")
      expect(html).toContain('data-linger-claim="no"')
    }
  })

  it("offers the choice, and a refusal names the manual command", () => {
    const offer = markup({ shown: "offer" })
    expect(offer).toContain("Keep it running")
    expect(offer).toContain("Only while I’m signed in")
    expect(offer).not.toContain("administrator")
    const refused = markup({ shown: "refused" })
    expect(refused).toContain("Try again")
    expect(refused).toContain("Start using Nessa")
    expect(refused).toContain("loginctl enable-linger")
    const failed = markup({ shown: "failed" })
    expect(failed).toContain("loginctl enable-linger")
    expect(failed).not.toContain("keeps running")
  })
})
