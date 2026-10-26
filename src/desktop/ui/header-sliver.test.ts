// @vitest-environment jsdom
import { readFileSync } from "node:fs"
import { expect, it } from "vitest"

const artSheetPath = "../styles.css"
const paneSheetPath = "../workspace/ui/panes/panes.css"
const styles = readFileSync(new URL(artSheetPath, import.meta.url), "utf8")
const panes = readFileSync(new URL(paneSheetPath, import.meta.url), "utf8")
const rule = (sheet: string, start: string) => {
  const index = sheet.indexOf(start)
  if (index < 0) throw new Error(`missing header rule: ${start}`)
  return sheet.slice(sheet.lastIndexOf("}", index) + 1).split("}")[0] + "}"
}
// Include a legacy art inset if one returns, so full-width assertions reject it.
const artCorner = ".workspace[data-panes-alone]"
const sliverStyles =
  rule(styles, ".desktop-header[data-sliver] {") +
  (styles.includes(artCorner) ? rule(styles, artCorner) : "")
const headerStyles =
  rule(panes, ".workspace-pane-header {") +
  rule(panes, ".workspace[data-panes-alone]\n  .workspace-pane:is(") +
  rule(panes, '.workspace-window[data-rail="open"]')

for (const alone of [false, true])
  for (const flight of ["none", "another pane", "this pane"])
    for (const corner of ["actual", "preview", "away", "other"])
      for (const rail of [false, true])
        it(`keeps full-width art and inset controls: corner=${corner}, alone=${alone}, flight=${flight}, rail=${rail}`, () => {
          const sheet = document.createElement("style")
          sheet.textContent = sliverStyles + headerStyles
          const windowShell = document.createElement("div")
          windowShell.className = "workspace-window"
          windowShell.dataset.rail = rail ? "open" : "closed"
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
          pane.innerHTML =
            '<header class="workspace-pane-header">controls</header>' +
            '<div class="desktop-header" data-sliver>scene</div>'
          workspace.append(pane)
          const normalControls = document.createElement("header")
          normalControls.className = "workspace-pane-header"
          windowShell.append(workspace, normalControls)
          document.head.append(sheet)
          document.body.append(windowShell)
          try {
            // JSDOM retains CSS-variable destinations but does not resolve
            // the base padding shorthand. Compare release with the same
            // source rule outside a corner; browser sampling owns pixels.
            const normalPadding = getComputedStyle(normalControls).paddingLeft
            const controls = pane.querySelector("header")
            const art = pane.querySelector("[data-sliver]")
            if (!controls || !art) throw new Error("header fixture is incomplete")
            const reserved =
              (corner === "actual" || corner === "preview") &&
              (alone || flight === "this pane")
            // Selector destinations, not simulated animation geometry. The
            // maintained safe-area browser check samples the moving boxes.
            expect(getComputedStyle(art).left).toBe("0px")
            expect(getComputedStyle(art).width).toBe("100%")
            expect(getComputedStyle(art).visibility).toBe("visible")
            expect(getComputedStyle(controls).paddingLeft).toBe(
              reserved
                ? rail
                  ? "calc(var(--desktop-titlebar-safe-start) - var(--rail-width))"
                  : "var(--desktop-titlebar-safe-start)"
                : normalPadding,
            )
            pane.removeAttribute("data-split-flying")
            expect(getComputedStyle(art).left).toBe("0px")
            if (!alone) expect(getComputedStyle(controls).paddingLeft).toBe(normalPadding)
          } finally {
            windowShell.remove()
            sheet.remove()
          }
        })
