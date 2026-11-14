// @vitest-environment jsdom
import { renderToStaticMarkup } from "react-dom/server"
import { expect, it } from "vitest"
import { SidebarRow } from "../source-list/sidebar-row"
import { WorkingWords } from "./working-words"

const render = (node: React.ReactElement) => {
  const host = document.createElement("div")
  host.innerHTML = renderToStaticMarkup(node)
  return host
}

it("shimmers the words at work, with no turning arc beside them", () => {
  const host = render(<WorkingWords working>Working</WorkingWords>)
  expect(host.querySelector(".workspace-shimmer[data-working]")?.textContent).toBe(
    "Working",
  )
  expect(host.querySelector(".workspace-status")).toBeNull()
  expect(host.textContent).toBe("Working")
})

it("draws plain words when nothing works, and says Running to a reader only when asked", () => {
  expect(render(<WorkingWords working={false}>Idle</WorkingWords>).innerHTML).toBe("Idle")
  const spoken = render(
    <WorkingWords working spoken>
      A chat
    </WorkingWords>,
  )
  expect(spoken.querySelector(".workspace-spoken")?.textContent).toBe(" Running")
})

it("shows a running channel by its name, not a glyph at its end", () => {
  const host = render(
    <SidebarRow
      icon="channel"
      label="general"
      active={false}
      current={false}
      running
      onClick={() => {}}
    />,
  )
  expect(host.querySelector(".workspace-shimmer")?.textContent).toBe("general")
  expect(host.querySelector('.workspace-status[data-status="running"]')).toBeNull()
  expect(host.querySelector("[data-trailing]")).toBeNull()
  expect(host.querySelector("[aria-label]")?.getAttribute("aria-label")).toBe(
    "general Running",
  )
})
