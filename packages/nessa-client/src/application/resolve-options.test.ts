import { describe, expect, it } from "vitest"

import { NessaClient } from "../presentation/nessa-client.js"
import { resolveConnectOptions } from "./resolve-options.js"
import { DEV_AUTH_TOKEN, StageConfigError } from "./stage.js"

const base = {
  role: "surface" as const,
  surface: { kind: "panel" as const, instance: "test" },
  client: { id: "test", version: "0.1.0", platform: "node" },
}

describe("resolveConnectOptions", () => {
  it("defaults stage to dev with loopback url and dev-token", () => {
    const resolved = resolveConnectOptions(base, NessaClient.defaultUrl)
    expect(resolved.stage).toBe("dev")
    expect(resolved.url).toBe("ws://127.0.0.1:7420")
    expect(resolved.auth.token).toBe(DEV_AUTH_TOKEN)
  })

  it("honours explicit url and token in dev", () => {
    const resolved = resolveConnectOptions(
      {
        ...base,
        stage: "dev",
        url: "ws://127.0.0.1:9999",
        auth: { token: "custom" },
      },
      NessaClient.defaultUrl,
    )
    expect(resolved.url).toBe("ws://127.0.0.1:9999")
    expect(resolved.auth.token).toBe("custom")
  })

  it("requires url and token outside dev", () => {
    expect(() =>
      resolveConnectOptions({ ...base, stage: "ci" }, NessaClient.defaultUrl),
    ).toThrow(StageConfigError)

    expect(() =>
      resolveConnectOptions(
        { ...base, stage: "ci", url: "ws://127.0.0.1:7420" },
        NessaClient.defaultUrl,
      ),
    ).toThrow(/auth\.token/)

    const resolved = resolveConnectOptions(
      {
        ...base,
        stage: "ci",
        url: "ws://127.0.0.1:7420",
        auth: { token: "ci-token" },
      },
      NessaClient.defaultUrl,
    )
    expect(resolved.auth.token).toBe("ci-token")
  })
})
