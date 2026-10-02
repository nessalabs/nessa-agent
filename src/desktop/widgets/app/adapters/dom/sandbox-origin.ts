/**
 * Where this window's sandbox proxy is (`SandboxOrigin`), for composition to
 * hand the app views: in the desktop app, the `nessa-sandbox` scheme the host
 * serves it on (`src-tauri/src/app_sandbox.rs`) — a host of its own on
 * Windows, where WebView2 serves a custom scheme as an `http` host — and in
 * the browser build, the listener the dev server started for it, named in
 * the page (`sandbox/serve.ts`).
 *
 * A page that names none, or names something that is not an `http(s)` URL
 * on another origin than its own, has no sandbox: every app in it is one the
 * window cannot load, said as such.
 */
import type { HostKind } from "../../../../../host/features"
import type { SandboxOrigin } from "../../application/ports"
import type { PageContext } from "../../model/host-context"

/** The meta element the browser build's dev server writes the proxy's URL into. */
export const sandboxMetaName = "nessa-app-sandbox"

/**
 * The proxy for a window whose host is `host` (`src/host`'s kinds): the
 * page's own listener in a browser; the scheme in the desktop app — as an
 * `http` host on the one host that is neither macOS nor Linux, Windows.
 */
export function sandboxFor(
  host: HostKind,
  document: Document,
): SandboxOrigin | undefined {
  switch (host) {
    case "browser":
      return pageSandbox(document)
    case "other":
      return {
        url: "http://nessa-sandbox.localhost/proxy.html",
        origin: "http://nessa-sandbox.localhost",
      }
    case "macos":
    case "linux":
      return {
        url: "nessa-sandbox://localhost/proxy.html",
        origin: "nessa-sandbox://localhost",
      }
  }
}

/** The proxy the page names, if it names one this page may use. */
export function pageSandbox(document: Document): SandboxOrigin | undefined {
  const named = document
    .querySelector(`meta[name="${sandboxMetaName}"]`)
    ?.getAttribute("content")
  if (!named) return undefined
  let url: URL
  try {
    url = new URL(named)
  } catch {
    return undefined
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return undefined
  // On the page's own origin the proxy would be no boundary at all.
  if (url.origin === document.location.origin) return undefined
  return { url: url.href, origin: url.origin }
}

/** What an app is told it runs on (`hostContext.platform`): the desktop app, or a browser. */
export function platformFor(host: HostKind): PageContext["platform"] {
  return host === "browser" ? "web" : "desktop"
}
