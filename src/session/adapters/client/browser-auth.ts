import { NessaRpcError } from "@nessa/client"
const pendingKey = "nessa.browser.logout-pending"
type StoragePort = Pick<Storage, "getItem" | "setItem" | "removeItem">

/** Same-origin browser API. Storage contains only logout intent, never credentials. */
export function createBrowserAuth(request: typeof fetch, storage: StoragePort) {
  let logoutPending = false
  function rememberLogout(pending: boolean) {
    logoutPending = pending
    try {
      if (pending) storage.setItem(pendingKey, "1")
      else storage.removeItem(pendingKey)
    } catch {
      /* Local teardown and server sign-out still work when storage is disabled. */
    }
  }
  function hasPendingLogout() {
    try {
      return logoutPending || Boolean(storage.getItem(pendingKey))
    } catch {
      return logoutPending
    }
  }
  async function send(path: string, token?: string) {
    return request(`/browser/${path}`, {
      method: "POST",
      credentials: "same-origin",
      signal: AbortSignal.timeout(10_000),
      headers: { "Content-Type": "application/json", "X-Nessa-Browser": "1" },
      ...(token === undefined ? {} : { body: JSON.stringify({ token }) }),
    })
  }
  async function logout() {
    rememberLogout(true)
    if (!(await send("logout")).ok)
      throw new Error("Server sign-out could not be confirmed. Please retry.")
    rememberLogout(false)
  }
  return {
    async restore() {
      try {
        if (hasPendingLogout()) {
          await logout()
          return false
        }
        const response = await send("check")
        if (response.status === 401) return false
        if (!response.ok) throw new Error("Session service unavailable")
        return true
      } catch {
        throw new NessaRpcError(
          "temporarily_unavailable",
          "Cannot restore sign-in right now. Please retry.",
        )
      }
    },
    async login(token: string) {
      const response = await send("login", token.trim())
      if (!response.ok)
        throw new Error(
          response.status === 401 ? "Access token was rejected." : "Unable to sign in.",
        )
      rememberLogout(false)
    },
    logout,
  }
}

/** Match the SDK transport policy before submitting any credential over HTTP. */
export function browserSessionUrl(
  pageUrl: string,
  stage: "dev" | "ci" | "alpha" | "prod",
): string {
  const url = new URL(pageUrl)
  const local =
    (stage === "dev" || stage === "ci") && ["127.0.0.1", "[::1]"].includes(url.hostname)
  if (
    url.username ||
    url.password ||
    (url.protocol !== "https:" && !(url.protocol === "http:" && local))
  )
    throw new Error("Use HTTPS, or HTTP on 127.0.0.1 in development, to sign in.")
  return `${url.protocol === "https:" ? "wss:" : "ws:"}//${url.host}/browser/session`
}
