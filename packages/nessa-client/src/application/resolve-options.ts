import type { NessaClientConnectOptions } from "./options.js"
import {
  DEV_AUTH_TOKEN,
  StageConfigError,
  stageAllowsDefaultAuth,
  type Stage,
} from "./stage.js"

export type ResolvedConnectOptions = NessaClientConnectOptions & {
  stage: Stage
  url: string
  auth: { token: string }
}

/**
 * Apply stage policy: `dev` gets loopback URL + default token; other stages
 * require explicit `url` and `auth.token`.
 */
export function resolveConnectOptions(
  options: NessaClientConnectOptions,
  defaultUrl: string,
): ResolvedConnectOptions {
  const stage: Stage = options.stage ?? "dev"

  const url = options.url ?? (stageAllowsDefaultAuth(stage) ? defaultUrl : undefined)
  if (!url) {
    throw new StageConfigError(
      `url is required for the ${stage} stage (dev may omit it and use ${defaultUrl})`,
    )
  }

  const token =
    options.auth?.token ?? (stageAllowsDefaultAuth(stage) ? DEV_AUTH_TOKEN : undefined)
  if (!token) {
    throw new StageConfigError(
      `auth.token is required for the ${stage} stage (dev may omit it and use the default credential)`,
    )
  }

  return {
    ...options,
    stage,
    url,
    auth: { token },
  }
}
