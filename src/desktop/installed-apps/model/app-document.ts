/**
 * One document from a verified static app package, for the sandbox proxy to
 * load into the app's opaque frame (`widgets/app/sandbox/proxy.html`).
 *
 * The frame has no origin to fetch the package's other files from, and its
 * policy (`cspPolicy` with nothing declared) allows no network at all, so
 * every file the entrypoint names is put into the document itself: a script
 * or a stylesheet inline, anything else (an image, a font) as a `data:`
 * address. A reference to a file the package does not hold is left as it is
 * and loads nothing. Nothing here decides what is allowed: the policy does.
 *
 * ```text
 * entrypoint HTML ──parse──▶ <script src>, <link rel=stylesheet>, src=/href= of a held file
 *     ──▶ inline text or data: address ──▶ one HTML string
 * ```
 */

/** A package file as the host hands it: its path and its bytes in base64. */
export interface HeldFile {
  readonly path: string
  readonly bytes: string
}

const base = "https://package.invalid/"

const types: Readonly<Record<string, string>> = {
  css: "text/css",
  js: "text/javascript",
  mjs: "text/javascript",
  json: "application/json",
  svg: "image/svg+xml",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  ico: "image/x-icon",
  woff: "font/woff",
  woff2: "font/woff2",
  ttf: "font/ttf",
  txt: "text/plain",
}

/** The media type a file is given by its extension; unknown ones are bytes. */
export function mediaType(path: string): string {
  const dot = path.lastIndexOf(".")
  const extension = dot < 0 ? "" : path.slice(dot + 1).toLowerCase()
  return Object.hasOwn(types, extension) ? types[extension] : "application/octet-stream"
}

function text(bytes: string): string {
  const binary = atob(bytes)
  const octets = Uint8Array.from(binary, (char) => char.charCodeAt(0))
  return new TextDecoder().decode(octets)
}

/** The package path a reference in the entrypoint names, or none outside it. */
function resolved(reference: string, entrypoint: string): string | undefined {
  let url: URL
  try {
    url = new URL(reference, base + entrypoint)
  } catch {
    return undefined
  }
  if (url.origin !== new URL(base).origin) return undefined
  return decodeURIComponent(url.pathname.slice(1))
}

/**
 * `css` with each `url(...)` that names a packaged file put inside it as a
 * `data:` URI, resolved against `from`, the stylesheet's own package path,
 * as a browser resolves it. A URL naming nothing in the package is left.
 */
function inlineCssUrls(
  css: string,
  from: string,
  held: ReadonlyMap<string, string>,
): string {
  return css.replace(
    /url\(\s*(["']?)([^"')]+)\1\s*\)/gi,
    (whole, _quote: string, reference: string) => {
      const path = resolved(reference.trim(), from)
      const bytes = path === undefined ? undefined : held.get(path)
      return bytes === undefined || path === undefined
        ? whole
        : `url("data:${mediaType(path)};base64,${bytes}")`
    },
  )
}

/**
 * The entrypoint with every file it names put inside it. `parse` is the
 * page's own HTML parser (`DOMParser`), passed in so a test can use its own.
 */
export function packageDocument(
  entrypoint: string,
  files: readonly HeldFile[],
  parse: (html: string) => Document,
): string {
  const held = new Map(files.map((file) => [file.path, file.bytes]))
  const entry = held.get(entrypoint)
  if (entry === undefined) throw new Error("The package holds no entrypoint")
  const page = parse(text(entry))
  const find = (reference: string | null) => {
    if (reference === null) return undefined
    const path = resolved(reference, entrypoint)
    return path === undefined || path === entrypoint ? undefined : held.get(path)
  }
  for (const script of Array.from(page.querySelectorAll("script[src]"))) {
    const bytes = find(script.getAttribute("src"))
    if (bytes === undefined) continue
    script.removeAttribute("src")
    // A closing tag inside the script would end it early in the document.
    script.textContent = text(bytes).replace(/<\/(script)/gi, "<\\/$1")
  }
  for (const link of Array.from(page.querySelectorAll('link[rel~="stylesheet"][href]'))) {
    const href = link.getAttribute("href")
    const bytes = find(href)
    if (bytes === undefined || href === null) continue
    const style = page.createElement("style")
    style.dataset.inlined = ""
    // Its own url(...)s are relative to the stylesheet, not the page.
    const from = resolved(href, entrypoint) ?? entrypoint
    style.textContent = inlineCssUrls(text(bytes), from, held).replace(
      /<\/(style)/gi,
      "<\\/$1",
    )
    link.replaceWith(style)
  }
  // A page's own <style> blocks resolve against the page.
  for (const style of Array.from(page.querySelectorAll("style"))) {
    if (style.dataset.inlined !== undefined) continue
    style.textContent = inlineCssUrls(style.textContent ?? "", entrypoint, held)
  }
  for (const attribute of ["src", "href"]) {
    for (const element of Array.from(page.querySelectorAll(`[${attribute}]`))) {
      if (element.localName === "a") continue
      const reference = element.getAttribute(attribute)
      const bytes = find(reference)
      if (bytes === undefined || reference === null) continue
      const path = resolved(reference, entrypoint) ?? ""
      element.setAttribute(attribute, `data:${mediaType(path)};base64,${bytes}`)
    }
  }
  return page.documentElement.outerHTML
}
