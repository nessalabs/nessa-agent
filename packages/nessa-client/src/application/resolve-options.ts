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
 * True for ws/wss URLs whose host is loopback.
 * Throws {@link StageConfigError} for non-WebSocket or unparseable URLs.
 */
export function isLoopbackWebSocketUrl(url: string): boolean {
  let parsed: URL
  try {
    parsed = new URL(url)
  } catch {
    throw new StageConfigError(`invalid WebSocket url: ${url}`)
  }
  if (parsed.protocol !== "ws:" && parsed.protocol !== "wss:") {
    throw new StageConfigError(`url must use ws: or wss: (got ${parsed.protocol})`)
  }
  const host = parsed.hostname.replace(/^\[|\]$/g, "")
  return host === "127.0.0.1" || host === "::1" || host === "localhost"
}

/**
 * Apply stage policy:
 * - `dev` may omit `url` (loopback default) and omit `auth.token` **only** for
 *   loopback URLs (default `dev-token`).
 * - Non-loopback URLs always require an explicit token, even in `dev`.
 * - Non-dev requires explicit `url` + `auth.token`.
 * - Non-dev non-loopback URLs must use `wss:`.
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

  const loopback = isLoopbackWebSocketUrl(url)

  if (!stageAllowsDefaultAuth(stage) && !loopback && !url.startsWith("wss:")) {
    throw new StageConfigError(`non-loopback ${stage} urls must use wss: (got ${url})`)
  }

  let token = options.auth?.token
  if (token === undefined) {
    if (stageAllowsDefaultAuth(stage) && loopback) {
      token = DEV_AUTH_TOKEN
    } else if (stageAllowsDefaultAuth(stage)) {
      throw new StageConfigError(
        `auth.token is required for non-loopback urls even in the ${stage} stage`,
      )
    } else {
      throw new StageConfigError(
        `auth.token is required for the ${stage} stage (dev may omit it on loopback)`,
      )
    }
  }

  return {
    ...options,
    stage,
    url,
    auth: { token },
  }
}
