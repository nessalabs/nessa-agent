import {
  initSync,
  isAuthorized,
  validate,
  type Schema,
} from "@cedar-policy/cedar-wasm/web"
import engine from "./generated/cedar.wasm"
import policy from "./policies.cedar"
import schema from "./schema.json"
import type { GitHubIdentity, RelayPolicyEvaluator } from "../../application/ports.js"

initSync({ module: engine })
const policies = { staticPolicies: { "github-current-repository-access": policy } }
const checked = validate({
  schema: schema as Schema,
  policies,
  validationSettings: { mode: "strict" },
})
if (
  checked.type !== "success" ||
  checked.validationErrors.length ||
  checked.validationWarnings.length ||
  checked.otherWarnings.length ||
  !policy.includes('@id("github-current-repository-access")')
)
  throw new Error("invalid relay policy")

export class CedarRelayPolicyEvaluator implements RelayPolicyEvaluator {
  read(identity: GitHubIdentity, repositoryId: string) {
    const user = { type: "Relay::User", id: identity.userId }
    const repo = { type: "Relay::Repository", id: repositoryId }
    const result = isAuthorized({
      principal: user,
      action: { type: "Relay::Action", id: "read" },
      resource: repo,
      context: {},
      schema: schema as Schema,
      validateRequest: true,
      policies,
      entities: [
        {
          uid: user,
          attrs: {
            installationActive: identity.installationActive ?? false,
            installationPermissions: [...(identity.installationPermissions ?? [])],
            appRepositories: (identity.appRepositoryIds ?? []).map((id) => ({
              __entity: { type: "Relay::Repository", id },
            })),
            oauthScopes: [...identity.oauthScopes],
            repositories: identity.repositoryIds.map((id) => ({
              __entity: { type: "Relay::Repository", id },
            })),
          },
          parents: [],
        },
        ...identity.repositoryIds.map((id) => ({
          uid: { type: "Relay::Repository", id },
          attrs: { isPrivate: identity.repositoryIsPrivate },
          parents: [],
        })),
      ],
    })
    if (
      result.type !== "success" ||
      result.response.diagnostics.errors.length ||
      result.warnings.length
    )
      return { allowed: false, rules: ["evaluation_failed"] }
    return {
      allowed: result.response.decision === "allow",
      rules: result.response.diagnostics.reason.length
        ? result.response.diagnostics.reason
        : ["default_deny"],
    }
  }
}
