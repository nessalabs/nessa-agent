import { NessaClientConfig } from "./client-config.js"
import type { NessaClientConnectOptions, ProductConnectOptions } from "./options.js"
import { StageConfigError, stageAllowsDefaultUrl, type Stage } from "./stage.js"

export type ResolvedProductConnectOptions = ProductConnectOptions & {
  auth: { credential: string }
  config: NessaClientConfig
  stage: Stage
  url: string
  profile: "product"
}
export type ResolvedConnectOptions = ResolvedProductConnectOptions

function productUrl(url: string): string {
  const parsed = new URL(url)
  const path = parsed.pathname.replace(/\/$/, "")
  parsed.pathname = path.endsWith("/session") ? path : `${path}/session`
  return parsed.toString()
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
 * Validate stage/URL policy and resolve defaults. Product credentials are always
 * loaded before resolution.
 * Non-dev requires an explicit URL and non-loopback connections require wss.
 */
export function resolveConnectOptions(
  options: NessaClientConnectOptions,
  defaultUrl: string,
): ResolvedConnectOptions {
  const config = options.config ?? new NessaClientConfig()
  const stage: Stage = options.stage ?? "dev"

  const url = options.url ?? (stageAllowsDefaultUrl(stage) ? defaultUrl : undefined)
  if (!url) {
    throw new StageConfigError(
      `url is required for the ${stage} stage (dev may omit it and use ${defaultUrl})`,
    )
  }

  const loopback = isLoopbackWebSocketUrl(url)

  if (!stageAllowsDefaultUrl(stage) && !loopback && !url.startsWith("wss:")) {
    throw new StageConfigError(`non-loopback ${stage} urls must use wss: (got ${url})`)
  }

  {
    const credential = options.auth?.credential
    const credentialBytes = new TextEncoder().encode(credential ?? "").byteLength
    if (
      credential === undefined ||
      credentialBytes === 0 ||
      credentialBytes > 16 * 1024
    ) {
      throw new StageConfigError(
        "auth.credential must contain 1 to 16384 bytes for product sessions",
      )
    }
    return {
      ...options,
      profile: "product",
      stage,
      url: productUrl(url),
      auth: { credential },
      config,
    }
  }
}
