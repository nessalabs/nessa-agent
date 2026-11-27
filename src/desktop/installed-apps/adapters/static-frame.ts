/**
 * Draws one static app's document through the MCP Apps sandbox proxy
 * (`widgets/app/sandbox/proxy.html`), the boundary MCP Apps already use:
 *
 * ```text
 * this page ──frames──▶ proxy (sandbox origin) ──frames──▶ the app (srcdoc, opaque origin)
 *   proxy-ready ◀───────────┘
 *   resource-ready {html, policy, checkWithin} ──▶ proxy applies the policy, loads the app
 *   app-left ◀───────────── the app's document went (navigated, reloaded, closed)
 * ```
 *
 * A static app is given no bridge and no tools: nothing it says is relayed
 * to anything. Its policy is the one an app that declared nothing gets
 * (`cspPolicy` of no domains): no network, no frames, no forms.
 */
import { proxyFrameSandbox } from "../../widgets/app/adapters/dom/sandbox-origin"
import type { SandboxOrigin } from "../../widgets/app/application/ports"
import { appDocument, cspPolicy, type AppliedCsp } from "../../widgets/app/model/csp"
import { sandboxMethods } from "../../widgets/app/model/sandbox-methods"

/** What a static app may reach: nothing. */
export const noNetwork: AppliedCsp = { connect: [], resource: [], baseUri: [] }

/** How long the app's document has to answer the proxy's check after each load. */
const checkWithinMs = 5_000

export interface StaticFrame {
  /** Removes the frame and stops listening. */
  remove(): void
}

export function mountStaticApp(
  container: HTMLElement,
  sandbox: SandboxOrigin,
  html: string,
  options: { readonly title: string; readonly onLeft?: () => void },
): StaticFrame {
  const page = container.ownerDocument.defaultView
  const frame = container.ownerDocument.createElement("iframe")
  frame.src = sandbox.url
  frame.setAttribute("sandbox", proxyFrameSandbox)
  frame.setAttribute("referrerpolicy", "no-referrer")
  frame.title = options.title
  frame.dataset.installedAppFrame = ""
  let sent = false
  const listener = (event: MessageEvent) => {
    if (event.source !== frame.contentWindow || event.origin !== sandbox.origin) return
    const data: unknown = event.data
    const method =
      typeof data === "object" && data !== null && "method" in data
        ? (data as { method: unknown }).method
        : undefined
    if (method === sandboxMethods.proxyReady && !sent) {
      sent = true
      frame.contentWindow?.postMessage(
        {
          jsonrpc: "2.0",
          method: sandboxMethods.resourceReady,
          params: {
            html: appDocument(html, noNetwork),
            policy: cspPolicy(noNetwork),
            checkWithin: checkWithinMs,
          },
        },
        sandbox.origin,
      )
    }
    if (method === sandboxMethods.appLeft) options.onLeft?.()
  }
  page?.addEventListener("message", listener)
  container.appendChild(frame)
  return {
    remove() {
      page?.removeEventListener("message", listener)
      frame.remove()
    },
  }
}
