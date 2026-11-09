import { URL } from "node:url"
import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"

/** Deployment names and secrets are per stage. Missing rollout data is a refusal,
 * never an invitation to inherit the development database or namespace. */
export function deployment(stage, targets) {
  if (!Object.hasOwn(targets, stage) || targets[stage] === null)
    throw new Error(`GitHub relay stage ${stage} has not been provisioned`)
  const target = targets[stage]
  if (
    target.vars.RELAY_STAGE !== stage ||
    !target.account_id ||
    !target.d1_databases?.[0]?.database_id ||
    !target.ratelimits?.some((binding) => binding.name === "EDGE_LIMIT") ||
    !target.ratelimits?.some((binding) => binding.name === "INGRESS_LIMIT")
  )
    throw new Error("Incomplete stage deployment")
  for (const [other, configuration] of Object.entries(targets)) {
    if (
      other !== stage &&
      configuration &&
      (configuration.name === target.name ||
        configuration.d1_databases[0].database_id ===
          target.d1_databases[0].database_id ||
        (configuration.account_id === target.account_id &&
          configuration.ratelimits?.some((binding) =>
            target.ratelimits.some((own) => own.namespace_id === binding.namespace_id),
          )))
    )
      throw new Error(
        "Relay stages must not share a Worker, database or rate-limit namespace",
      )
  }
  return target
}
export function configuredDevelopment() {
  const path = fileURLToPath(new URL("../wrangler.jsonc", import.meta.url))
  const text = readFileSync(path, "utf8")
  // Wrangler's checked-in file has trailing commas, with no comment syntax.
  return JSON.parse(text.replace(/,\s*([}\]])/g, "$1"))
}

export function stageConfiguration(stage, targets) {
  if (!Object.hasOwn(targets, stage) || !targets[stage])
    throw new Error(`Unconfigured relay stage ${stage}`)
  const origins = new Set(),
    apps = new Set(),
    clients = new Set(),
    appIds = new Set()
  for (const value of Object.values(targets)) {
    if (!value) continue
    if (!Number.isSafeInteger(value.appId) || value.appId <= 0)
      throw new Error("A stage requires a GitHub App ID")
    const origin = new URL(value.origin)
    if (
      origin.protocol !== "https:" ||
      origin.pathname !== "/" ||
      origin.search ||
      origin.hash ||
      origin.username ||
      origin.password
    )
      throw new Error("Invalid relay origin")
    if (
      origins.has(origin.origin) ||
      apps.has(value.appSlug) ||
      clients.has(value.clientId) ||
      appIds.has(value.appId)
    )
      throw new Error("Stages require separate relays and GitHub Apps")
    origins.add(origin.origin)
    apps.add(value.appSlug)
    clients.add(value.clientId)
    appIds.add(value.appId)
  }
  return targets[stage]
}
