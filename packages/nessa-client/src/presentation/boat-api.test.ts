import { describe, expect, it, vi } from "vitest"
import { boatStatus, createBoatApi } from "./boat-api.js"

describe("Boat connection", () => {
  it("uses the authenticated connection and sends only the token for setup", async () => {
    const connected = { state: "connected", environment: "nessa", image: "fixture-image" }
    const request = vi.fn(async () => connected)
    const boat = createBoatApi({ request })
    await boat.save({ token: "fixture-token" })
    await boat.status()
    await boat.signIn()
    await boat.cancelSignIn()
    await boat.disconnect()
    expect(request.mock.calls).toEqual([
      ["boat.save", { token: "fixture-token" }],
      ["boat.status", {}],
      ["boat.signIn", {}],
      ["boat.cancelSignIn", {}],
      ["boat.disconnect", {}],
    ])
  })

  it("rejects credential-bearing, partial and inherited status projections", () => {
    for (const value of [
      {
        state: "connected",
        token: "must-not-be-returned",
        environment: "nessa",
        image: "fixture-image",
      },
      { state: "connected", environment: "nessa" },
      { state: "locked", image: "fixture-image" },
      Object.create({ state: "disconnected" }),
      { state: "connected", environment: "nessa", image: null },
    ])
      expect(() => boatStatus(value)).toThrow(TypeError)
    for (const value of [
      { state: "disconnected", login: "octo" },
      {
        state: "connected",
        environment: "nessa",
        image: "i",
        via: "token",
        login: "octo",
      },
      { state: "connected", environment: "nessa", image: "i", via: "password" },
      { state: "disconnected", signIn: { state: "waiting", url: "javascript:alert(1)" } },
      {
        state: "disconnected",
        signIn: { state: "waiting", url: "https://boat.dev", code: "x" },
      },
      { state: "disconnected", signInAvailable: "yes" },
    ])
      expect(() => boatStatus(value)).toThrow(TypeError)
    expect(
      boatStatus({
        state: "connected",
        environment: "nessa",
        image: "i",
        via: "signIn",
        login: "octo",
        signInAvailable: true,
      }),
    ).toEqual({
      state: "connected",
      environment: "nessa",
      image: "i",
      via: "signIn",
      login: "octo",
      signInAvailable: true,
    })
    expect(
      boatStatus({
        state: "disconnected",
        signIn: {
          state: "waiting",
          url: "https://boat.dev/api/boat/oauth2/authorize?x=1",
        },
      }),
    ).toEqual({
      state: "disconnected",
      signIn: { state: "waiting", url: "https://boat.dev/api/boat/oauth2/authorize?x=1" },
    })
    expect(boatStatus({ state: "locked" })).toEqual({ state: "locked" })
    expect(boatStatus({ state: "disconnected" })).toEqual({ state: "disconnected" })
  })
})

it("discards untrusted provider diagnostics and secret parameters on failure", async () => {
  const { NessaRpcError } = await import("../application/rpc-error.js")
  const { NessaBoatError } = await import("./boat-api.js")
  const boat = createBoatApi({
    request: async () => {
      throw new NessaRpcError("boat_environment_blocks_credentials", "fixture-secret", {
        token: "fixture-secret",
      })
    },
  })
  const error = await boat
    .save({ token: "fixture-secret" })
    .catch((value: unknown) => value)
  expect(error).toBeInstanceOf(NessaBoatError)
  expect(String(error)).not.toContain("fixture-secret")
  expect(JSON.stringify(error)).not.toContain("fixture-secret")
  expect(error).toMatchObject({ refusal: "boat_environment_blocks_credentials" })
})
