/**
 * Gateway invitation scope. The ordinary panel keeps its session. An explicit
 * owner token opens a separate, non-reconnecting session to its pinned endpoint;
 * the registry decides Human eligibility when create is called. The view gets
 * only pairing methods, and closes the scope on navigation or its deadline.
 * gateway-pairing.test.ts covers identity, teardown and late-connect boundaries.
 */
import {
  NessaClient,
  NessaClientConfig,
  PairingEnrollee,
  type PairingApi,
  type Stage,
} from "@nessa/client"

export interface PairingScope {
  readonly pairing: Pick<
    PairingApi,
    "create" | "pending" | "status" | "approve" | "cancel"
  >
  close(): void
}

export interface GatewayPairing {
  readonly after: (ms: number, run: () => void) => () => void
  open(token: string | undefined, signal: AbortSignal): Promise<PairingScope>
}

type Panel = Pick<NessaClient, "auth" | "pairing"> & { readonly sessionUrl?: string }

export function gatewayPairing(options: {
  readonly connected: () => Promise<Panel>
  readonly stage: Stage
  readonly connect: typeof NessaClient.connect
  readonly newId: () => string
  readonly after: GatewayPairing["after"]
}): GatewayPairing {
  return {
    after: options.after,
    async open(token, signal) {
      let privileged: NessaClient | undefined
      let pairing: PairingApi | undefined
      const close = () => {
        pairing = undefined
        signal.removeEventListener("abort", close)
        privileged?.close()
        privileged = undefined
      }
      signal.addEventListener("abort", close, { once: true })
      const check = () => {
        if (signal.aborted) throw new DOMException("Pairing closed", "AbortError")
      }
      try {
        check()
        const panel = await options.connected()
        check()
        pairing = panel.pairing
        if (token !== undefined) {
          const endpoint = panel.sessionUrl
          if (!endpoint) throw new Error("Gateway is reconnecting")
          const expected = await panel.auth.session()
          check()
          if (panel.sessionUrl !== endpoint)
            throw new Error("Gateway changed during sign-in; continue again")
          const url = new URL(endpoint)
          if (url.pathname === "/browser/session") url.pathname = "/session"
          const client = await options.connect({
            profile: "product",
            stage: options.stage,
            url: url.toString(),
            auth: { credential: token },
            role: "surface",
            surface: { kind: "desktop", instance: options.newId() },
            client: { id: "nessa-pairing", version: "0.1.0", platform: "browser" },
            config: new NessaClientConfig({
              retry: { maxAttempts: 1 },
              reconnect: { enabled: false },
            }),
          })
          // An abort while connect was pending cannot close a client not yet
          // returned. Retire that late result before exposing any command.
          privileged = client
          check()
          const actual = await client.auth.session()
          check()
          if (
            actual.gatewayId !== expected.gatewayId ||
            actual.organizationId !== expected.organizationId ||
            actual.audienceId !== expected.audienceId
          )
            throw new Error("Owner sign-in reached a different gateway")
          pairing = client.pairing
        }
        const run = async <T>(call: (api: PairingApi) => Promise<T>): Promise<T> => {
          check()
          const api = pairing
          if (!api) throw new DOMException("Pairing closed", "AbortError")
          return call(api)
        }
        // No general client or credential reaches React. Do not automatically
        // replay a mutation: a lost answer is resolved with pending/status.
        return {
          close,
          pairing: {
            create: (params) => run((api) => api.create(params)),
            pending: () => run((api) => api.pending()),
            status: (id) => run((api) => api.status(id)),
            approve: (id, key) => run((api) => api.approve(id, key)),
            cancel: (id) => run((api) => api.cancel(id)),
          },
        }
      } catch (error) {
        close()
        throw error
      }
    },
  }
}

/** True and false are both explicit creation choices, never status fallbacks. */
export function gatewayInvitation(samePerson: boolean) {
  return { enrollee: PairingEnrollee.Gateway, samePerson }
}
