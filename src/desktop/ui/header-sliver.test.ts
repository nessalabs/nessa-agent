// @vitest-environment jsdom
import { readFileSync } from "node:fs"
import { expect, it } from "vitest"

const sheetPath = "../styles.css"
const styles = readFileSync(new URL(sheetPath, import.meta.url), "utf8")
const rule = (start: string) => {
  const index = styles.indexOf(start)
  if (index < 0) throw new Error(`missing sliver rule: ${start}`)
  return styles.slice(index).split("}")[0] + "}"
}
const corner = styles.indexOf(".workspace[data-panes-alone]")
if (corner < 0) throw new Error("missing corner sliver rule")
const cornerRule = styles.slice(styles.lastIndexOf("}", corner) + 1).split("}")[0] + "}"
const sliverStyles = rule(".desktop-header[data-sliver] {") + cornerRule

for (const alone of [false, true])
  for (const flight of ["none", "another pane", "this pane"])
    for (const corner of ["actual", "preview", "away", "other"])
      it(`reserves the controls for the ${corner} corner: alone=${alone}, flight=${flight}`, () => {
        const sheet = document.createElement("style")
        sheet.textContent = sliverStyles
        const workspace = document.createElement("main")
        workspace.className = "workspace"
        workspace.toggleAttribute("data-panes-alone", alone)
        workspace.toggleAttribute("data-split-flipping", flight !== "none")
        const pane = document.createElement("article")
        pane.className = "workspace-pane"
        pane.toggleAttribute("data-split-flying", flight === "this pane")
        pane.toggleAttribute(
          "data-split-corner",
          corner === "actual" || corner === "away",
        )
        if (corner === "preview" || corner === "away")
          pane.dataset.dragCorner = corner === "preview" ? "yes" : "no"
        pane.innerHTML = '<div class="desktop-header" data-sliver>scene</div>'
        workspace.append(pane)
        document.head.append(sheet)
        document.body.append(workspace)
        try {
          const header = pane.firstElementChild
          if (!header) throw new Error("sliver fixture is incomplete")
          const reserved =
            (corner === "actual" || corner === "preview") &&
            (alone || flight === "this pane")
          const computed = getComputedStyle(header)
          // Actual selector destinations, not simulated animation geometry.
          // Safe-area browser sampling owns the moving painted boxes.
          expect(computed.left).toBe(
            reserved ? "var(--desktop-titlebar-safe-start)" : "0px",
          )
          expect(computed.width).toBe(reserved ? "auto" : "100%")
          expect(computed.visibility).toBe("visible")
          pane.removeAttribute("data-split-flying")
          if (!alone) expect(getComputedStyle(header).left).toBe("0px")
        } finally {
          workspace.remove()
          sheet.remove()
        }
      })
