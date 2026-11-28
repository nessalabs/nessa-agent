import { expect, it } from "vitest"
import { createAppsApi } from "./apps-api.js"

function requester(answers: unknown[]) {
  const asked: [string, unknown][] = []
  return {
    asked,
    session: {
      request: async (method: string, params: unknown) => {
        asked.push([method, params])
        return answers.shift()
      },
    },
  }
}

it("lists and untrusts apps on their own methods", async () => {
  const { asked, session } = requester([
    { servers: ["charts", "huddle"] },
    { requestId: "r1", applied: true },
  ])
  const apps = createAppsApi(session as never)
  await expect(apps.trusted()).resolves.toEqual(["charts", "huddle"])
  await expect(apps.untrust("huddle", "r1")).resolves.toBe(true)
  expect(asked).toEqual([
    ["apps.trusted", {}],
    ["apps.untrust", { requestId: "r1", server: "huddle" }],
  ])
})

it("refuses answers that are not the documented shape", async () => {
  const { session } = requester([
    { servers: [""] },
    { servers: ["a"], extra: 1 },
    { requestId: "other", applied: true },
  ])
  const apps = createAppsApi(session as never)
  await expect(apps.trusted()).rejects.toThrow()
  await expect(apps.trusted()).rejects.toThrow()
  await expect(apps.untrust("huddle", "r1")).rejects.toThrow()
})

const digest = "a".repeat(64)
const review = {
  appId: "notes",
  packageVersion: "1.0.0",
  entrypoint: "index.html",
  assetCount: 1,
  digest,
}
const pkg = { manifest: "e30=", assets: [{ path: "index.html", bytes: "PHA+" }] }

it("makes, reviews, installs, lists, reads and removes installed apps", async () => {
  const { asked, session } = requester([
    { package: pkg, review },
    review,
    { requestId: "r1", app: { appId: "notes", packageVersion: "1.0.0" } },
    { apps: [{ appId: "notes", packageVersion: "1.0.0" }] },
    { package: pkg, review },
    { requestId: "r2", appId: "notes" },
  ])
  const apps = createAppsApi(session as never)
  await expect(apps.starter("notes", "1.0.0")).resolves.toEqual({ package: pkg, review })
  await expect(apps.review(pkg)).resolves.toEqual(review)
  await expect(apps.install(pkg, digest, "r1")).resolves.toEqual({
    appId: "notes",
    packageVersion: "1.0.0",
  })
  await expect(apps.installed()).resolves.toEqual([
    { appId: "notes", packageVersion: "1.0.0" },
  ])
  await expect(apps.read("notes")).resolves.toEqual({ package: pkg, review })
  await expect(apps.uninstall("notes", "r2")).resolves.toBeUndefined()
  expect(asked).toEqual([
    ["apps.starter", { appId: "notes", packageVersion: "1.0.0" }],
    ["apps.review", { package: pkg }],
    ["apps.install", { requestId: "r1", package: pkg, reviewedDigest: digest }],
    ["apps.installed", {}],
    ["apps.read", { appId: "notes" }],
    ["apps.uninstall", { requestId: "r2", appId: "notes" }],
  ])
})

it("refuses installed-app answers that are not the documented shape", async () => {
  const { session } = requester([
    { ...review, digest: "short" },
    { requestId: "r1", app: { appId: "notes" } },
    { apps: [], more: true },
    { package: pkg, review: { ...review, appId: "other" } },
    { requestId: "r2", appId: "other" },
  ])
  const apps = createAppsApi(session as never)
  await expect(apps.review(pkg)).rejects.toThrow()
  await expect(apps.install(pkg, digest, "r1")).rejects.toThrow()
  await expect(apps.installed()).rejects.toThrow()
  await expect(apps.read("notes")).rejects.toThrow()
  await expect(apps.uninstall("notes", "r2")).rejects.toThrow()
})
