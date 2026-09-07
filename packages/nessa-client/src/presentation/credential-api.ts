import { NessaMutationError } from "../application/mutation-error.js"
import { ProductMethod } from "../protocol/product-types.js"
import type {
  CredentialIssueParams,
  CredentialListResult,
  CredentialRevokeResult,
  ExistingCredentialResult,
  IssuedCredentialResult,
  ProductCredentialMetadata,
  ProductGrant,
  ProductPrincipal,
} from "../protocol/product-types.js"
import type { RpcRequester } from "../application/session-port.js"

/** SDK alias for {@link ProductGrant}: an action restricted to an organization-scoped resource. */
export type CredentialGrant = ProductGrant
/** SDK alias for {@link ProductCredentialMetadata}; contains no credential secret. */
export type CredentialMetadata = ProductCredentialMetadata
/** Credential issuance input. Specify a principal, its membership, and requested grants. An optional requestId makes caller-managed retries possible. */
export type IssueCredentialParams = Omit<CredentialIssueParams, "requestId"> & {
  /** Generated if omitted. Reuse this value for an explicit retry of the same operation. */
  requestId?: string
}
/** Issuance result plus the operation requestId. Narrow using "secret" in result: first issuance returns the secret, duplicate issuance returns secretUnavailable instead. */
export type IssueCredentialResult = (
  IssuedCredentialResult | ExistingCredentialResult
) & { requestId: string }
export type PrincipalKind = ProductPrincipal["kind"]

/** Authorized product credential management on the client connection. Mutations are never automatically replayed. Failures are wrapped in NessaMutationError so callers can retain requestId and inspect cause before an explicit retry. */
export type CredentialApi = {
  /** Issue once; retries with the same requestId return metadata without replaying the secret. */
  issue: (params: IssueCredentialParams) => Promise<IssueCredentialResult>
  /** List authorized credential metadata without secret evidence. */
  list: () => Promise<CredentialListResult>
  /** Revoke once. Reuse requestId after an uncertain failure; omit to generate a UUID. */
  revoke: (
    credentialId: string,
    requestId?: string,
  ) => Promise<CredentialRevokeResult & { requestId: string }>
}

function record(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("credential response is not an object")
  }
  return value as Record<string, unknown>
}

function metadata(value: unknown): CredentialMetadata {
  const item = record(value)
  for (const field of ["id", "principalId", "organizationId", "audienceId"] as const) {
    if (typeof item[field] !== "string" || item[field].length === 0) {
      throw new Error(`credential response missing ${field}`)
    }
  }
  for (const field of ["issuedAt"] as const) {
    if (!Number.isSafeInteger(item[field]) || (item[field] as number) < 0) {
      throw new Error(`credential response has invalid ${field}`)
    }
  }
  if (
    item.expiresAt !== null &&
    (!Number.isSafeInteger(item.expiresAt) ||
      (item.expiresAt as number) <= (item.issuedAt as number))
  ) {
    throw new Error("credential response has invalid lifetime")
  }
  if (
    item.revokedAt !== null &&
    (!Number.isSafeInteger(item.revokedAt) || (item.revokedAt as number) < 0)
  ) {
    throw new Error("credential response has invalid revokedAt")
  }
  if (!Array.isArray(item.grants))
    throw new Error("credential response has invalid grants")
  for (const grantValue of item.grants) {
    const grant = record(grantValue)
    const resource = record(grant.resource)
    if (
      typeof grant.action !== "string" ||
      grant.action.length === 0 ||
      typeof resource.organizationId !== "string" ||
      resource.organizationId.length === 0 ||
      typeof resource.id !== "string" ||
      resource.id.length === 0 ||
      resource.organizationId !== item.organizationId
    ) {
      throw new Error("credential response has invalid grant")
    }
  }
  return item as unknown as CredentialMetadata
}

function issueResult(value: unknown): IssuedCredentialResult | ExistingCredentialResult {
  const result = record(value)
  const credential = metadata(result.credential)
  if (typeof result.secret === "string" && result.secret.length > 0) {
    return { credential, secret: result.secret }
  }
  if (result.secretUnavailable === true) return { credential, secretUnavailable: true }
  throw new Error("credential issue response contains no usable secret status")
}

export function createCredentialApi(
  session: RpcRequester,
  newRequestId: () => string,
): CredentialApi {
  async function mutate<T>(
    requestId: string,
    perform: () => Promise<T>,
  ): Promise<T & { requestId: string }> {
    try {
      return { ...(await perform()), requestId }
    } catch (error) {
      throw new NessaMutationError(requestId, error)
    }
  }
  return {
    issue: async (params) => {
      const requestId = params.requestId ?? newRequestId()
      return mutate(requestId, async () =>
        issueResult(
          await session.request(ProductMethod.CredentialIssue, { ...params, requestId }),
        ),
      )
    },
    list: async () => {
      const result = record(await session.request(ProductMethod.CredentialList, {}))
      if (!Array.isArray(result.credentials))
        throw new Error("credential list response is invalid")
      return { credentials: result.credentials.map(metadata) }
    },
    revoke: async (credentialId, requestId = newRequestId()) =>
      mutate(requestId, async () => {
        const result = record(
          await session.request(ProductMethod.CredentialRevoke, {
            requestId,
            credentialId,
          }),
        )
        if (
          result.credentialId !== credentialId ||
          !Number.isSafeInteger(result.revision) ||
          (result.revision as number) < 0
        ) {
          throw new Error("credential revoke response is invalid")
        }
        return result as unknown as CredentialRevokeResult
      }),
  }
}
