import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"
import { ToolPayload } from "./tool-payload"

const render = (text: string) =>
  renderToStaticMarkup(createElement(ToolPayload, { text }))
describe("tool payload representation", () => {
  it("uses the shared tree for objects, arrays, null and falsy JSON", () => {
    for (const text of [
      '{"command":"ls"}',
      '[1,{"nested":true}]',
      "null",
      "false",
      "0",
    ]) {
      expect(render(text)).toContain('data-slot="json-tree"')
    }
  })
  it("decodes stream escapes once while retaining metadata and escaping HTML", () => {
    const html = render(
      JSON.stringify({
        stdout: "  first\n<script>alert(1)</script>\n\\n",
        stderr: "",
        exitCode: 0,
      }),
    )
    expect(html).toContain("  first\n&lt;script&gt;alert(1)&lt;/script&gt;\n\\n")
    expect(html).toContain("exitCode")
    expect(html).toContain('aria-label="stderr"')
    expect(html).not.toContain("<script>")
  })
  it("preserves incomplete JSON and plain output without interpreting markup", () => {
    for (const text of ['  {"partial":', "  output\nnext", "<b>literal</b>"]) {
      const html = render(text)
      expect(html).not.toContain('data-slot="json-tree"')
      expect(html).toContain("<pre")
    }
    expect(render("  output\nnext")).toContain("  output\nnext")
  })
})
