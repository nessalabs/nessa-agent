/**
 * The packaged app's `connect-src`, for a browser page that stands in for the
 * app's window.
 *
 * The packaged window enforces `src-tauri/tauri.conf.json`'s content policy,
 * and neither `tauri dev` nor the Vite page a check drives does (`devCsp` is
 * null). A fetch the policy blocks therefore passed every browser check and
 * failed only in the app: reading a pasted image back from its object URL
 * was refused there, so no image was ever sent. A page given this policy
 * fails the same way the app does.
 *
 * Only `connect-src` is taken: the dev page's own scripts are inline Vite
 * preambles the packaged `script-src` would refuse, which says nothing about
 * the app. `also` adds the origins only the check has, such as its
 * gateway's own port, which the packaged app reaches at a fixed one.
 */
import { readFileSync } from "node:fs"
import { join } from "node:path"

import { repoRoot } from "../../../../scripts/mcp-test-server/local-gateway.mjs"

/** The packaged `connect-src` sources, read from the one file that sets them. */
export function packagedConnectSources() {
  const config = JSON.parse(
    readFileSync(join(repoRoot, "src-tauri/tauri.conf.json"), "utf8"),
  )
  const directive = String(config.app.security.csp)
    .split(";")
    .map((part) => part.trim().split(/\s+/))
    .find(([name]) => name === "connect-src")
  if (!directive) throw new Error("the packaged content policy has no connect-src")
  return directive.slice(1)
}

/** The `Content-Security-Policy` header value a stand-in window is served with. */
export function appConnectPolicy(also = []) {
  return `connect-src ${[...packagedConnectSources(), ...also].join(" ")}`
}

/**
 * Puts `policy` on every window document `context` loads, as the packaged
 * app's webview applies its own to the page it loads.
 *
 * As a `<meta>` policy, prepended to the head the moment the parser makes
 * it, before any of the page's scripts run. Not as a response header: a
 * document the harness answers itself counts as public to Chromium's
 * local network checks, which then block every loopback connection the
 * page makes. A frame inside the window keeps its own policy, as it does in
 * the app.
 */
export async function enforcePolicy(context, policy) {
  await context.addInitScript((content) => {
    if (window.top !== window) return
    const put = () => {
      if (!document.head) return false
      const meta = document.createElement("meta")
      meta.httpEquiv = "Content-Security-Policy"
      meta.content = content
      document.head.prepend(meta)
      return true
    }
    if (put()) return
    const watch = new MutationObserver(() => {
      if (put()) watch.disconnect()
    })
    watch.observe(document, { childList: true, subtree: true })
  }, policy)
}

/**
 * Whether the window `page` shows is held to `policy`: its head carries it,
 * and a connection the policy does not allow is refused as a violation of
 * it. Throws otherwise, so a check never passes because the policy silently
 * did not apply in an engine.
 */
export async function assertPolicyHolds(page, policy) {
  const held = await page.evaluate(async (content) => {
    const meta = [
      ...document.head.querySelectorAll('meta[http-equiv="Content-Security-Policy"]'),
    ].some((each) => each.content === content)
    const violated = new Promise((resolve) => {
      const on = (event) => {
        if (event.violatedDirective.startsWith("connect-src")) {
          document.removeEventListener("securitypolicyviolation", on)
          resolve(true)
        }
      }
      document.addEventListener("securitypolicyviolation", on)
      setTimeout(() => resolve(false), 5000)
    })
    // Port 9 (discard) on loopback: not in the policy, and nothing answers.
    const refused = await fetch("http://127.0.0.1:9/policy-probe").then(
      () => false,
      () => true,
    )
    return { meta, refused, violated: await violated }
  }, policy)
  if (!held.meta || !held.refused || !held.violated)
    throw new Error(
      `the window is not held to the packaged connect-src: ${JSON.stringify(held)}`,
    )
}
