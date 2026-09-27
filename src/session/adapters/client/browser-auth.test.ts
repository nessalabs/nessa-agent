import { expect, it, vi } from "vitest"
import { createBrowserAuth, browserSessionUrl } from "./browser-auth"
it("restores through an HttpOnly cookie without requesting or storing a token", async () => {
  const request = vi
    .fn<typeof fetch>()
    .mockResolvedValue(new Response(null, { status: 204 }))
  const auth = createBrowserAuth(request, memoryStorage())
  expect(await auth.restore()).toBe(true)
  await auth.logout()
  expect(request.mock.calls.map(([url]) => url)).toEqual([
    "/browser/check",
    "/browser/logout",
  ])
  expect(request.mock.calls[0]?.[1]).toMatchObject({
    method: "POST",
    credentials: "same-origin",
    headers: { "X-Nessa-Browser": "1" },
  })
  expect(request.mock.calls[0]?.[1]?.body).toBeUndefined()
})
it("sends the submitted token only to login and treats expired sessions as signed out", async () => {
  const request = vi
    .fn<typeof fetch>()
    .mockResolvedValueOnce(new Response(null, { status: 204 }))
    .mockResolvedValue(new Response(null, { status: 401 }))
  const auth = createBrowserAuth(request, memoryStorage())
  await auth.login(" test ")
  expect(request.mock.calls[0]?.[1]?.body).toBe('{"token":"test"}')
  expect(await auth.restore()).toBe(false)
  await expect(auth.logout()).rejects.toThrow("sign-out could not be confirmed")
  await expect(auth.login("wrong")).rejects.toThrow("rejected")
})

function memoryStorage() {
  const data = new Map<string, string>()
  return {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => {
      data.set(key, value)
    },
    removeItem: (key: string) => {
      data.delete(key)
    },
  }
}
it("keeps a logout fence across refresh when the response is lost", async () => {
  const storage = memoryStorage()
  const request = vi
    .fn<typeof fetch>()
    .mockRejectedValueOnce(new Error("lost response"))
    .mockResolvedValue(new Response(null, { status: 204 }))
  const first = createBrowserAuth(request, storage)
  await expect(first.logout()).rejects.toThrow("lost response")
  const refreshed = createBrowserAuth(request, storage)
  expect(await refreshed.restore()).toBe(false)
  expect(request.mock.calls.map(([path]) => path)).toEqual([
    "/browser/logout",
    "/browser/logout",
  ])
  expect(await refreshed.restore()).toBe(true)
})
it("keeps unavailable session checks retryable instead of interpreting them as logout", async () => {
  const request = vi
    .fn<typeof fetch>()
    .mockResolvedValue(new Response(null, { status: 503 }))
  await expect(
    createBrowserAuth(request, memoryStorage()).restore(),
  ).rejects.toMatchObject({ code: "temporarily_unavailable" })
})

it("selects a same-origin socket and rejects insecure non-development locations", () => {
  for (const stage of ["dev", "ci", "alpha", "prod"] as const) {
    for (const host of [
      "127.0.0.1",
      "[::1]",
      "localhost",
      "example.com",
      "127.0.0.1.evil.example",
    ]) {
      expect(browserSessionUrl(`https://${host}:1443/`, stage)).toBe(
        `wss://${host}:1443/browser/session`,
      )
      if (["dev", "ci"].includes(stage) && ["127.0.0.1", "[::1]"].includes(host))
        expect(browserSessionUrl(`http://${host}:1420/`, stage)).toBe(
          `ws://${host}:1420/browser/session`,
        )
      else expect(() => browserSessionUrl(`http://${host}:1420/`, stage)).toThrow()
    }
  }
  expect(() => browserSessionUrl("file:///index.html", "dev")).toThrow()
})
