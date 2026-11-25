import { describe, expect, it } from "vitest"
import { NessaRpcError } from "../application/rpc-error.js"
import { agent37Status, createAgent37Api } from "./agent37-api.js"
import { cloudChoice, createCloudApi } from "./cloud-api.js"

describe("agent37 api", () => {
  it("reads only the statuses the gateway sends", () => {
    expect(agent37Status({ state: "locked" })).toEqual({ state: "locked" })
    expect(agent37Status({ state: "connected", template: "nessa-runtime@1" })).toEqual({
      state: "connected",
      template: "nessa-runtime@1",
    })
    for (const bad of [
      { state: "connected" },
      { state: "disconnected", template: "x" },
      { state: "connected", template: "x", key: "sk_live_leak" },
      null,
    ])
      expect(() => agent37Status(bad)).toThrow(TypeError)
    expect(cloudChoice({ cloud: "agent37" })).toEqual({ cloud: "agent37" })
    expect(() => cloudChoice({ cloud: "elsewhere" })).toThrow(TypeError)
  })

  it("sends the key once and reports a refusal by its code alone", async () => {
    const sent: [string, unknown][] = []
    const api = createAgent37Api({
      request: async (method: string, params: unknown) => {
        sent.push([method, params])
        if (method === "agent37.save")
          throw new NessaRpcError(
            "agent37_unavailable",
            "the key sk_live_secret was refused",
          )
        return { cloud: "agent37" }
      },
    } as never)
    const cloud = createCloudApi({
      request: async (method: string, params: unknown) => {
        sent.push([method, params])
        if ((params as { cloud?: string }).cloud === "boat")
          throw new NessaRpcError("cloud_not_configured", "Boat is not connected")
        return { cloud: "agent37" }
      },
    } as never)
    await expect(api.save({ key: "sk_live_secret" })).rejects.toMatchObject({
      name: "NessaAgent37Error",
      refusal: "agent37_unavailable",
      message: "Agent37 operation refused (agent37_unavailable)",
    })
    expect(await cloud.choose("agent37")).toEqual({ cloud: "agent37" })
    await expect(cloud.choose("boat")).rejects.toMatchObject({
      name: "NessaCloudError",
      refusal: "cloud_not_configured",
    })
    expect(sent).toEqual([
      ["agent37.save", { key: "sk_live_secret" }],
      ["cloud.choose", { cloud: "agent37" }],
      ["cloud.choose", { cloud: "boat" }],
    ])
  })

  it("validates cloud.boxes lifecycle responses and redacts errors to typed refusal codes", async () => {
    const sample = {
      boxId: "11111111-2222-3333-4444-555555555555",
      name: "Cloud Box",
      cloud: "agent37",
      vm: "vm-1",
      url: "wss://box.example.com/",
      state: "running",
      createdAtMs: 100,
      updatedAtMs: 200,
    }
    const seenOptions = new Map<string, unknown>()
    const cloud = createCloudApi({
      request: async (method: string, _params?: unknown, options?: unknown) => {
        seenOptions.set(method, options)
        switch (method) {
          case "cloud.boxes.list":
            return { boxes: [sample] }
          case "cloud.boxes.create":
            return { cloudBox: sample, credential: "enroll-once" }
          case "cloud.boxes.wake":
            return {
              boxId: sample.boxId,
              url: "wss://rotated.example.com/",
            }
          case "cloud.boxes.sleep":
            throw new NessaRpcError("cloud_box_active_work", "active terminal pid=42")
          case "cloud.boxes.rename":
            return { cloudBox: { ...sample, name: "Prod Box" } }
          case "cloud.boxes.delete":
            return { boxId: sample.boxId, deleted: true }
          default:
            throw new Error(`unexpected ${method}`)
        }
      },
    } as never)

    expect(await cloud.boxes.list()).toEqual([sample])
    expect(
      await cloud.boxes.create({
        boxId: sample.boxId,
        name: "Cloud Box",
        cloud: "agent37",
      }),
    ).toEqual({
      cloudBox: sample,
      credential: "enroll-once",
    })
    expect(await cloud.boxes.wake(sample.boxId)).toEqual({
      boxId: sample.boxId,
      url: "wss://rotated.example.com/",
    })
    await expect(cloud.boxes.sleep(sample.boxId)).rejects.toMatchObject({
      name: "NessaCloudBoxError",
      refusal: "cloud_box_active_work",
      message: "Cloud box refused (cloud_box_active_work)",
    })
    expect((await cloud.boxes.rename(sample.boxId, "Prod Box")).name).toBe("Prod Box")
    expect(await cloud.boxes.delete(sample.boxId)).toEqual({
      boxId: sample.boxId,
      deleted: true,
    })
    expect(seenOptions.get("cloud.boxes.create")).toEqual({ atLeastMs: 240_000 })
    expect(seenOptions.get("cloud.boxes.wake")).toEqual({ atLeastMs: 150_000 })
    expect(seenOptions.get("cloud.boxes.sleep")).toEqual({ atLeastMs: 60_000 })
    expect(seenOptions.get("cloud.boxes.delete")).toEqual({ atLeastMs: 60_000 })
  })
})
