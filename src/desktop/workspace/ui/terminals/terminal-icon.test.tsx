// @vitest-environment jsdom
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"
import { ClockProvider } from "../../../../clock"
import { TerminalIcon, TerminalStatusIndicator } from "./terminal-icon"

function renderHtml(node: React.ReactElement): HTMLElement {
  const host = document.createElement("div")
  host.innerHTML = renderToStaticMarkup(
    <ClockProvider now={() => 0} elapsed={() => 0}>
      {node}
    </ClockProvider>,
  )
  return host
}

describe("TerminalIcon", () => {
  it("renders Claude and Codex badges and falls back to the terminal glyph", () => {
    const claude = renderHtml(<TerminalIcon agent="claude" size={18} />)
    expect(claude.querySelector('[data-terminal-agent="claude"] svg path')).not.toBeNull()

    const codex = renderHtml(<TerminalIcon agent="codex" size={18} />)
    expect(codex.querySelector('[data-terminal-agent="codex"] svg path')).not.toBeNull()

    const fallback = renderHtml(<TerminalIcon agent={null} size={18} />)
    expect(fallback.querySelector("[data-terminal-agent]")).toBeNull()
    expect(fallback.querySelector("svg")).not.toBeNull()
  })
})

describe("TerminalStatusIndicator", () => {
  it("maps OSC 7501 states to status indicators", () => {
    const blocked = renderHtml(
      <TerminalStatusIndicator
        status={{ state: "blocked", kind: "permission", updatedAtMs: 1 }}
      />,
    )
    expect(blocked.querySelector('[data-status="needs-you"]')).not.toBeNull()

    const done = renderHtml(
      <TerminalStatusIndicator status={{ state: "done", updatedAtMs: 1 }} />,
    )
    expect(done.querySelector('[data-status="unread"]')).not.toBeNull()

    const error = renderHtml(
      <TerminalStatusIndicator
        status={{ state: "error", msg: "Build failed", updatedAtMs: 1 }}
      />,
    )
    expect(error.querySelector('[data-status="error"]')).not.toBeNull()

    const idle = renderHtml(
      <TerminalStatusIndicator status={{ state: "idle", updatedAtMs: 1 }} />,
    )
    expect(idle.firstElementChild).toBeNull()
  })
})
