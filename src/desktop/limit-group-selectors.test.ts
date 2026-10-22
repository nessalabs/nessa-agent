import { describe, expect, it } from "vitest"

import { limitGroupSelectors } from "./limit-group-selectors"

describe("limitGroupSelectors", () => {
  it("names the utility as the subject of a hovered group", () => {
    const css =
      ".group-hover\\/copy\\:opacity-100:is(:where(.group\\/copy):hover *){opacity:1}"
    expect(limitGroupSelectors(css)).toBe(
      ":is(:where(.group\\/copy):hover .group-hover\\/copy\\:opacity-100){opacity:1}",
    )
  })

  it("keeps an attribute on the ancestor, outside :where", () => {
    const css =
      ".group-\\[\\[data-preview-suppressed\\=false\\]\\:hover\\]\\/rail-item\\:opacity-100:is(:where(.group\\/rail-item)[data-preview-suppressed=false]:hover *){opacity:1}"
    expect(limitGroupSelectors(css)).toBe(
      ":is(:where(.group\\/rail-item)[data-preview-suppressed=false]:hover .group-\\[\\[data-preview-suppressed\\=false\\]\\:hover\\]\\/rail-item\\:opacity-100){opacity:1}",
    )
  })

  it("rewrites each selector in a list", () => {
    const css =
      ".group-hover\\:ml-0:is(:where(.group):hover *),.group-focus-within\\/copy\\:opacity-100:is(:where(.group\\/copy):focus-within *){}"
    expect(limitGroupSelectors(css)).toBe(
      ":is(:where(.group):hover .group-hover\\:ml-0),:is(:where(.group\\/copy):focus-within .group-focus-within\\/copy\\:opacity-100){}",
    )
  })

  it("leaves a sibling universal selector in place", () => {
    const css = ".peer-checked\\:opacity-100:is(:where(.peer):checked~*){opacity:1}"
    expect(limitGroupSelectors(css)).toBe(css)
  })

  it("leaves a rule that does not use the universal selector", () => {
    const css = ".workspace-pane:hover .workspace-pane-actions{opacity:1}"
    expect(limitGroupSelectors(css)).toBe(css)
  })
})
