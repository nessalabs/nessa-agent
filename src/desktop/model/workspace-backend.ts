/**
 * Whether the window shows a gateway's conversations or the sample workspace,
 * from what the page was opened with. A browser preview shows the gateway's
 * when its address asks (`?gateway`), over the gateway session this origin
 * already signed in to; anything else shows the sample, which is what the
 * verification fixtures open. The desktop host's own window does not ask yet:
 * the host hands its gateway credential to the panel and setup windows only
 * (#248).
 */
export function gatewayRequested(search: string): boolean {
  return new URLSearchParams(search).has("gateway")
}
