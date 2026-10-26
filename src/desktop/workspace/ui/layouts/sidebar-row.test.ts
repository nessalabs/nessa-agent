// @vitest-environment jsdom
import { readFileSync } from "node:fs"
import { expect, it } from "vitest"

const paths = [
  "../source-list/source-list.css",
  "../session-list/session-list.css",
  "./layouts.css",
]
const styles = paths
  .map((path) => readFileSync(new URL(path, import.meta.url), "utf8"))
  .join("\n")

for (const folding of [false, true])
  for (const held of [false, true])
    for (const listHeld of [false, true])
      it(`retains only the folding sidebar row: folding=${folding}, spare=${held}, list=${listHeld}`, () => {
        const sheet = document.createElement("style")
        sheet.textContent = styles
        const workspace = document.createElement("main")
        workspace.className = "workspace"
        workspace.dataset.sidebar = "closed"
        workspace.toggleAttribute("data-folding", folding)
        workspace.dataset.list = "open"
        workspace.toggleAttribute("data-spare-held", held)
        workspace.toggleAttribute("data-list-held", listHeld)
        workspace.innerHTML = `<aside class="workspace-sidebar"></aside>
          <section class="workspace-list"><div class="workspace-list-inner"><header class="desktop-column-bar"></header></div></section>
          <section class="workspace-overview-layer"></section>`
        document.head.append(sheet)
        document.body.append(workspace)
        try {
          const element = (selector: string) => {
            const found = workspace.querySelector(selector)
            if (!found) throw new Error(`missing ${selector}`)
            return getComputedStyle(found)
          }
          // These are actual stylesheet destinations; jsdom supplies no
          // animated geometry. The marker's real frame order is exercised
          // by layouts.test.tsx, and browser geometry is a separate gate.
          const retained = held && folding
          expect(element(".workspace-sidebar").position).toBe(
            retained ? "relative" : "absolute",
          )
          expect(element(".desktop-column-bar").paddingLeft).toBe("12px")
          expect(element(".workspace-overview-layer").left).toBe(
            retained
              ? "calc(var(--desktop-gutter) + var(--workspace-sidebar-width))"
              : "0px",
          )
          expect(element(".workspace-list-inner").paddingLeft).toBe(
            retained || listHeld ? "0" : "var(--desktop-gutter)",
          )
        } finally {
          workspace.remove()
          sheet.remove()
        }
      })
