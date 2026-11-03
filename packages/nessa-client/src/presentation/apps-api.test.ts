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
