import { NessaRpcError } from "./rpc-error.js"
import { StageConfigError } from "./stage.js"
import { NessaConnectionClosedError } from "./connection-closed-error.js"

/** Temporary transport failure. Never contains credential evidence. */
export class RetryableConnectError extends Error {}

/** Retry settings for initial product connection setup. */
export type ProductConnectRetryOptions = {
  /** Total attempts, including the first. Default 3; 1 disables retries. */
  maxAttempts?: number
  /** Initial backoff ceiling in milliseconds. Default 250. */
  initialDelayMs?: number
  /** Maximum backoff ceiling in milliseconds. Default 2_000. */
  maxDelayMs?: number
  /** Randomize each delay between half and all of its ceiling. Default true. */
  jitter?: boolean
}

/** Retry policy after defaults and validation; every setting is present. Used by NessaClientConfig. */
export type ResolvedConnectRetryOptions = Required<ProductConnectRetryOptions>

/** Validate before opening any socket. Timer values must fit platform timers. */
export function resolveConnectRetry(
  options: ProductConnectRetryOptions = {},
): ResolvedConnectRetryOptions {
  const resolved = {
    maxAttempts: options.maxAttempts ?? 3,
    initialDelayMs: options.initialDelayMs ?? 250,
    maxDelayMs: options.maxDelayMs ?? 2_000,
    jitter: options.jitter ?? true,
  }
  if (!Number.isSafeInteger(resolved.maxAttempts) || resolved.maxAttempts < 1) {
    throw new StageConfigError(
      "retry.maxAttempts must be a positive safe integer (1 disables retries)",
    )
  }
  for (const name of ["initialDelayMs", "maxDelayMs"] as const) {
    const value = resolved[name]
    if (!Number.isSafeInteger(value) || value < 0 || value > 2_147_483_647) {
      throw new StageConfigError(
        `retry.${name} must be an integer from 0 to 2147483647 milliseconds`,
      )
    }
  }
  if (resolved.maxDelayMs < resolved.initialDelayMs) {
    throw new StageConfigError("retry.maxDelayMs must be at least retry.initialDelayMs")
  }
  if (typeof resolved.jitter !== "boolean") {
    throw new StageConfigError("retry.jitter must be a boolean")
  }
  return resolved
}

export type ConnectRetryTiming = {
  wait: (milliseconds: number) => Promise<void>
  random: () => number
}

export function isRetryableConnectionError(error: unknown): boolean {
  return (
    error instanceof RetryableConnectError ||
    (error instanceof NessaConnectionClosedError && error.retryable) ||
    (error instanceof NessaRpcError && error.code === "temporarily_unavailable")
  )
}

/** Retry connection setup only. Each attempt must create a new socket. */
export async function retryProductConnection<T>(
  attempt: () => Promise<T>,
  timing: ConnectRetryTiming,
  policy: ResolvedConnectRetryOptions = resolveConnectRetry(),
): Promise<T> {
  for (let index = 0; ; index += 1) {
    try {
      return await attempt()
    } catch (error) {
      const transient = isRetryableConnectionError(error)
      if (!transient || index + 1 >= policy.maxAttempts) throw error
      // Cap the exponent as well, so even a very large attempt count stays finite.
      const ceiling = Math.min(
        policy.maxDelayMs,
        policy.initialDelayMs * 2 ** Math.min(index, 31),
      )
      await timing.wait(
        Math.max(
          policy.jitter ? ceiling / 2 + (timing.random() * ceiling) / 2 : ceiling,
          error instanceof NessaConnectionClosedError ? (error.retryAfterMs ?? 0) : 0,
        ),
      )
    }
  }
}
