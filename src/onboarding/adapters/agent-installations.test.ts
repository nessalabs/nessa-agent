import { expect, it, vi } from "vitest"
import { NessaRpcError, type AgentsApi } from "@nessa/client"
import { gatewayAgentInstallations } from "./agent-installations"

it.each([
  ["agent_download_failed", "download"],
  ["agent_download_refused", "refused"],
  ["agent_install_storage_failed", "storage"],
  ["agent_install_verification_failed", "verification"],
  ["agent_install_busy", "busy"],
  ["agent_install_unsupported", "unavailable"],
  ["agent_install_not_confirmed", "not-confirmed"],
])("maps %s from its typed code", async (code, reason) => {
  const api: AgentsApi = {
    list: vi.fn(),
    installOptions: vi.fn(),
    install: vi.fn().mockRejectedValue(new NessaRpcError(code, "same diagnostic")),
  }
  const source = gatewayAgentInstallations(
    () => api,
    () => "request",
    () => () => {},
  )
  expect(await source.install("claude")).toEqual({ status: "failed", reason })
  expect(api.install).toHaveBeenCalledWith("claude", "request")
})

it("does not mistake a transport failure or another agent's answer for confirmed installation", async () => {
  const api: AgentsApi = {
    list: vi.fn(),
    installOptions: vi.fn(),
    install: vi.fn().mockRejectedValue(new Error("lost connection")),
  }
  const source = gatewayAgentInstallations(
    () => api,
    () => "request",
    () => () => {},
  )
  expect(await source.install("claude")).toEqual({
    status: "failed",
    reason: "not-confirmed",
  })
  vi.mocked(api.install).mockResolvedValue({
    agent: "codex",
    version: "test",
    downloaded: true,
    cleanupPending: false,
  })
  expect(await source.install("claude")).toEqual({
    status: "failed",
    reason: "not-confirmed",
  })
})
