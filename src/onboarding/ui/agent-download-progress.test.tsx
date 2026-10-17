// @vitest-environment jsdom
import * as React from "react"
import { createRoot } from "react-dom/client"
import { expect, it } from "vitest"
import { AgentDownloadProgress } from "./agent-download-progress"

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })

it("keeps installation indeterminate when the archive offer is unavailable", async () => {
  const container = document.createElement("div")
  const root = createRoot(container)
  await React.act(async () => root.render(<AgentDownloadProgress name="Codex" />))
  const progress = container.querySelector('[role="progressbar"]')
  expect(progress?.getAttribute("aria-label")).toBe("Downloading and verifying Codex")
  expect(progress?.hasAttribute("aria-valuenow")).toBe(false)
  expect(container.textContent).not.toContain("Archive size")
  expect(container.textContent).toContain("installation will continue")
  await React.act(async () => root.unmount())
})
