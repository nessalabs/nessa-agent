import { expect, it, vi } from "vitest"
import { NessaVaultError, NessaRpcError, type VaultApi } from "@nessa/client"
import { gatewayVault } from "./gateway"
it("preserves committed-with-audit-failure without replay or status refresh", async () => {
  const request = vi.fn(async () => {
    throw new NessaVaultError(
      new NessaRpcError("vault_changed_audit_unavailable", "synthetic secret"),
    )
  })
  const vault: VaultApi = {
    changePassphrase: request,
    status: async () => ({ state: "locked" }),
    setup: async () => ({ state: "unlocked" }),
    unlock: async () => ({ state: "unlocked" }),
    lock: async () => ({ state: "locked" }),
  }
  const source = gatewayVault(async () => ({ vault }))
  await expect(
    source.changePassphrase("synthetic replacement phrase"),
  ).rejects.toMatchObject({ code: "vault_changed_audit_unavailable" })
  expect(request).toHaveBeenCalledTimes(1)
})
it("discards arbitrary failures as unconfirmed for a change", async () => {
  const connected = vi.fn(async () => {
    throw new Error("synthetic private transport text")
  })
  await expect(
    gatewayVault(connected).changePassphrase("synthetic replacement phrase"),
  ).rejects.toMatchObject({ code: "vault_change_not_confirmed" })
  expect(connected).toHaveBeenCalledTimes(1)
})
