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
  requestedCapabilities: ["list", "create"],
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

it("issues and revokes an app's credential on their own methods", async () => {
  const issued = {
    requestId: "c1",
    appId: "notes",
    credentialId: "app-1",
    credential: "app-1.secret",
    expiresAt: 99,
    capabilities: ["list", "create"],
  }
  const { asked, session } = requester([
    issued,
    { requestId: "c2", appId: "notes", revoked: 1 },
  ])
  const apps = createAppsApi(session as never)
  await expect(apps.issueCredential("notes", "c1")).resolves.toEqual(issued)
  await expect(apps.revokeCredentials("notes", "c2")).resolves.toBe(1)
  expect(asked).toEqual([
    ["apps.issueCredential", { requestId: "c1", appId: "notes" }],
    ["apps.revokeCredentials", { requestId: "c2", appId: "notes" }],
  ])
})

it("refuses credential answers that are not the documented shape", async () => {
  const issued = {
    requestId: "c1",
    appId: "notes",
    credentialId: "app-1",
    credential: "app-1.secret",
    expiresAt: 99,
    capabilities: ["list"],
  }
  const { session } = requester([
    { ...issued, capabilities: [] },
    { ...issued, capabilities: ["list", "list"] },
    { ...issued, capabilities: ["admin"] },
    { ...issued, credential: "" },
    { ...issued, appId: "other" },
    { requestId: "c2", appId: "notes", revoked: -1 },
    { ...review, requestedCapabilities: ["everything"] },
  ])
  const apps = createAppsApi(session as never)
  for (let index = 0; index < 5; index += 1)
    await expect(apps.issueCredential("notes", "c1")).rejects.toThrow()
  await expect(apps.revokeCredentials("notes", "c2")).rejects.toThrow()
  await expect(apps.review(pkg)).rejects.toThrow()
})
