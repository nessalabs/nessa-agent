import type { HostKind } from "../../host/features"

/**
 * Where the window's workspace comes from, by the host it runs in and what
 * the page was opened with:
 *
 * - `host`: the desktop app. The host serves the window the local gateway's
 *   endpoint and the panel's surface credential once its startup of the
 *   gateway is ready, as a reader that never starts one (`GatewayReader`,
 *   #419).
 * - `browser`: a browser preview opened with `?gateway`, over the gateway
 *   session this origin already signed in to. The preview does not sign in
 *   or renew the session itself: sign in through the panel's browser surface
 *   on the same origin, and when that session ends the window cannot reach
 *   the gateway until it does again.
 * - `sample`: any other browser page — the in-memory sample, which is what
 *   the verification fixtures open.
 */
export type WorkspaceBackend = "host" | "browser" | "sample"

export function workspaceBackend(host: HostKind, search: string): WorkspaceBackend {
  if (host !== "browser") return "host"
  return new URLSearchParams(search).has("gateway") ? "browser" : "sample"
}
