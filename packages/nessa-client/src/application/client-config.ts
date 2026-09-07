import {
  resolveConnectRetry,
  type ProductConnectRetryOptions,
  type ResolvedConnectRetryOptions,
} from "./connect-retry.js"
import { StageConfigError } from "./stage.js"

/** Inputs for {@link NessaClientConfig}. Omitted fields receive validated defaults; all durations are milliseconds. */
export type NessaClientConfigOptions = {
  /** Initial product-handshake retries. Does not replay established-session RPCs. */
  retry?: ProductConnectRetryOptions
  /** Reconnect established product sessions after transient closure. Enabled by default. */
  reconnect?: ProductConnectRetryOptions & { enabled?: boolean }
  /** Timeout for normal RPCs in milliseconds. Default 30_000. */
  requestTimeoutMs?: number
}

/** Validated, immutable client tuning. Safe to reuse across independent clients. */
export class NessaClientConfig {
  /** Initial product-handshake attempt and backoff policy. */
  readonly retry: Readonly<ResolvedConnectRetryOptions>
  /** Recovery policy after an established product connection is interrupted. */
  readonly reconnect: Readonly<ResolvedConnectRetryOptions & { enabled: boolean }>
  /** Deadline for a normal RPC response, in milliseconds. Timing out does not undo a mutation. */
  readonly requestTimeoutMs: number

  /** Apply defaults and freeze the configuration. Throws StageConfigError for invalid timer, attempt, or jitter values. A shared config contains no session or credential state. */
  constructor(options: NessaClientConfigOptions = {}) {
    const timeout = options.requestTimeoutMs ?? 30_000
    if (!Number.isSafeInteger(timeout) || timeout < 1 || timeout > 2_147_483_647) {
      throw new StageConfigError(
        "requestTimeoutMs must be an integer from 1 to 2147483647 milliseconds",
      )
    }
    this.requestTimeoutMs = timeout
    this.retry = Object.freeze(resolveConnectRetry(options.retry))
    const enabled = options.reconnect?.enabled ?? true
    if (typeof enabled !== "boolean")
      throw new StageConfigError("reconnect.enabled must be a boolean")
    this.reconnect = Object.freeze({ ...resolveConnectRetry(options.reconnect), enabled })
    Object.freeze(this)
  }
}
