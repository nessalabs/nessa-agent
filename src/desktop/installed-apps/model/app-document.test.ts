// @vitest-environment jsdom
import { describe, expect, it } from "vitest"
import { mediaType, packageDocument } from "./app-document"

const b64 = (text: string) => btoa(String.fromCharCode(...new TextEncoder().encode(text)))
const parse = (html: string) => new DOMParser().parseFromString(html, "text/html")

describe("packageDocument", () => {
  it("puts the files the entrypoint names inside it", () => {
    const html = packageDocument(
      "index.html",
      [
        {
          path: "index.html",
          bytes: b64(
            '<html><head><link rel="stylesheet" href="style.css"></head><body><img src="./img/dot.png"><script src="app.js"></script><a href="other.html">x</a></body></html>',
          ),
        },
        { path: "style.css", bytes: b64("body { color: red }") },
        { path: "app.js", bytes: b64('document.body.dataset.ran = "yes" // </script>') },
        { path: "img/dot.png", bytes: "iVBORw0K" },
        { path: "other.html", bytes: b64("<p>other</p>") },
      ],
      parse,
    )
    const page = parse(html)
    expect(page.querySelector("style")?.textContent).toBe("body { color: red }")
    expect(page.querySelector("link")).toBeNull()
    const script = page.querySelector("script")
    expect(script?.hasAttribute("src")).toBe(false)
    expect(script?.textContent).toContain('dataset.ran = "yes"')
    expect(script?.textContent).not.toContain("</script>")
    expect(page.querySelector("img")?.getAttribute("src")).toBe(
      "data:image/png;base64,iVBORw0K",
    )
    // A link is navigation, not a file to load: it is left alone.
    expect(page.querySelector("a")?.getAttribute("href")).toBe("other.html")
  })

  it("resolves a stylesheet's url()s against the stylesheet, and the page's against the page", () => {
    const html = packageDocument(
      "index.html",
      [
        {
          path: "index.html",
          bytes: b64(
            '<html><head><link rel="stylesheet" href="css/style.css"><style>p { background: url(img/dot.png) }</style></head><body></body></html>',
          ),
        },
        {
          path: "css/style.css",
          bytes: b64(
            "body { background: url('../img/dot.png') } h1 { background: url(https://example.com/x.png) }",
          ),
        },
        { path: "img/dot.png", bytes: "iVBORw0K" },
      ],
      parse,
    )
    const [sheet, own] = Array.from(parse(html).querySelectorAll("style"))
    expect(sheet?.textContent).toContain('url("data:image/png;base64,iVBORw0K")')
    // Outside the package: left, and the sandbox's policy refuses it.
    expect(sheet?.textContent).toContain("url(https://example.com/x.png)")
    expect(own?.textContent).toContain('url("data:image/png;base64,iVBORw0K")')
  })

  it("leaves a reference to a file outside the package as it is", () => {
    const html = packageDocument(
      "index.html",
      [
        {
          path: "index.html",
          bytes: b64(
            '<script src="https://example.com/x.js"></script><img src="../up.png">',
          ),
        },
      ],
      parse,
    )
    const page = parse(html)
    expect(page.querySelector("script")?.getAttribute("src")).toBe(
      "https://example.com/x.js",
    )
    expect(page.querySelector("img")?.getAttribute("src")).toBe("../up.png")
  })

  it("refuses a package without its entrypoint", () => {
    expect(() => packageDocument("index.html", [], parse)).toThrow()
  })

  it("types a file by its extension", () => {
    expect(mediaType("a/b.CSS")).toBe("text/css")
    expect(mediaType("blob")).toBe("application/octet-stream")
    expect(mediaType("x.constructor")).toBe("application/octet-stream")
  })
})
