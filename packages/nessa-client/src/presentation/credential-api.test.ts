import { expect, it, vi } from "vitest"
import { createCredentialApi, type IssueCredentialParams } from "./credential-api.js"
import { NessaMutationError } from "../application/mutation-error.js"
import { NessaConnectionClosedError } from "../application/connection-closed-error.js"

it("generates an ID once, exposes it on failure, and accepts it on explicit retry", async () => {
  const request = vi
    .fn()
    .mockRejectedValueOnce(new NessaConnectionClosedError(1006, ""))
    .mockResolvedValueOnce({ credentialId: "target", revision: 2 })
  const generate = vi.fn(() => "generated-id")
  const api = createCredentialApi({ request }, generate)
  const failure = await api.revoke("target").catch((error) => error)
  expect(failure).toBeInstanceOf(NessaMutationError)
  expect(failure).toMatchObject({ requestId: "generated-id", code: 1006 })
  expect(request).toHaveBeenCalledTimes(1)
  expect(await api.revoke("target", failure.requestId)).toMatchObject({
    requestId: "generated-id",
    revision: 2,
  })
  expect(generate).toHaveBeenCalledTimes(1)
  expect(request.mock.calls.map((call) => call[1])).toEqual([
    { credentialId: "target", requestId: "generated-id" },
    { credentialId: "target", requestId: "generated-id" },
  ])
})

it("adds a generated ID to issuance without mutating caller input or replaying requests", async () => {
  const request = vi.fn().mockRejectedValue(new Error("offline"))
  const api = createCredentialApi({ request }, () => "issue-id")
  const params: IssueCredentialParams = {
    principal: { id: "reader", kind: "agent" },
    membership: {
      id: "member",
      principalId: "reader",
      organizationId: "org",
      role: "member",
      state: "active",
    },
    expiresAt: 2000,
    grants: [],
  }
  await expect(api.issue(params)).rejects.toMatchObject({ requestId: "issue-id" })
  expect(params.requestId).toBeUndefined()
  expect(request).toHaveBeenCalledOnce()
  expect(request.mock.calls[0][1]).toEqual({ ...params, requestId: "issue-id" })
})
