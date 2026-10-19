import { describe, expect, it, vi } from "vitest"
import type { WorkspaceWindow } from "../generated/product.js"
import { createWorkspaceApi } from "./workspace-api.js"

const windowId = "7d9f2a4e-1b3c-4d5e-8f60-718293a4b5c6"

const window: WorkspaceWindow = {
  arrangement: "columns",
  sidebar: true,
  sessionList: true,
  showing: "panes",
  columns: [
    {
      widthPercent: 100,
      panes: [{ heightPercent: 100, focused: true, item: { kind: "draft" } }],
    },
  ],
}

describe("telling the gateway what the window shows", () => {
  it("publishes a window, and withdraws it with none", async () => {
    const request = vi.fn().mockResolvedValue({ revision: 2, applied: true })
    const api = createWorkspaceApi({ request })
    expect(await api.publish(windowId, 2, window)).toEqual({ revision: 2, applied: true })
    expect(request).toHaveBeenCalledWith("workspace.publish", {
      windowId,
      revision: 2,
      window,
    })
    await api.publish(windowId, 3)
    expect(request).toHaveBeenLastCalledWith("workspace.publish", {
      windowId,
      revision: 3,
    })
  })

  it("refuses an answer that is not the protocol's", async () => {
    for (const answer of [
      null,
      [],
      { revision: -1, applied: true },
      { revision: 1 },
      { revision: 1, applied: true, extra: 1 },
    ]) {
      const api = createWorkspaceApi({ request: vi.fn().mockResolvedValue(answer) })
      await expect(api.publish(windowId, 1, window)).rejects.toThrow(
        "Invalid workspace publish response",
      )
    }
  })
})
