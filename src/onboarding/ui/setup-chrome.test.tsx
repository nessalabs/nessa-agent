// @vitest-environment jsdom
import * as React from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"

import { SetupChrome } from "./setup-chrome"

function corners(props: React.ComponentProps<typeof SetupChrome>) {
  const html = renderToStaticMarkup(React.createElement(SetupChrome, props))
  const parsed = new DOMParser().parseFromString(html, "text/html")
  return [...parsed.querySelectorAll(".nessa-setup-corner")].map((corner) =>
    [...corner.querySelectorAll("button")].map((button) =>
      button.getAttribute("aria-label"),
    ),
  )
}

describe("setup's corner buttons", () => {
  it("puts Back top left, Close top right, and sound in the bottom corner", () => {
    expect(corners({ onBack: () => undefined, onClose: () => undefined })).toEqual([
      ["Back"],
      ["Close setup"],
      ["Turn setup sounds on"],
    ])
  })

  it("draws no Back where there is no step before, and keeps the left corner", () => {
    expect(corners({ onClose: () => undefined })).toEqual([
      [],
      ["Close setup"],
      ["Turn setup sounds on"],
    ])
  })
})
