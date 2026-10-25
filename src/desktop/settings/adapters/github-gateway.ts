/**
 * The gateway's GitHub sign-in, for Settings › Connections › Accounts: the
 * window's client (`client.github`) read into the model's words
 * (`model/github.ts`). Whether a sign-in may start, and how it ends, is the
 * gateway's to say by its answers. Each refusal and each way a sign-in ends
 * is mapped by a total table, so a code the protocol adds is a type error
 * here rather than a sentence nobody wrote.
 */
import {
  GithubConnectEnd,
  GithubRefusalCode,
  NessaGithubError,
  type GithubApi,
  type GithubConnection,
  type GithubRefusalCode as GithubRefusal,
} from "@nessa/client"
import type { GithubOutcome, GithubView } from "../model/github"

/** What Settings asks of the window's gateway client. */
export interface GithubClient {
  readonly github: GithubApi
}

/** The sign-in, as Settings manages it. */
export interface GithubGateway {
  /** How often the row reads again while a code waits. */
  readonly pollMs: number
  read(): Promise<GithubOutcome>
  connect(): Promise<GithubOutcome>
  cancel(): Promise<GithubOutcome>
  disconnect(): Promise<GithubOutcome>
}

const refusals: Record<GithubRefusal, string> = {
  [GithubRefusalCode.GithubNotConfigured]:
    "GitHub sign-in isn't available on this gateway.",
  [GithubRefusalCode.GithubUnreachable]:
    "Couldn't reach GitHub. Check the connection and try again.",
  [GithubRefusalCode.GithubRejected]:
    "GitHub refused the Nessa app. Update Nessa and try again.",
  [GithubRefusalCode.GithubStorageUnavailable]:
    "Couldn't remove the sign-in. It is still in use; try again.",
  [GithubRefusalCode.GithubAuditUnavailable]:
    "Couldn't keep a record of the change. Try again.",
  [GithubRefusalCode.Forbidden]:
    "This window isn't allowed to change the GitHub sign-in.",
  [GithubRefusalCode.Unauthorized]:
    "This window isn't allowed to change the GitHub sign-in.",
  [GithubRefusalCode.InvalidRequest]: "The gateway didn't understand the request.",
  [GithubRefusalCode.TemporarilyUnavailable]: "The gateway is busy. Try again.",
}

/** Why the last sign-in ended, one sentence each. */
const endedBecause: Record<GithubConnectEnd, string> = {
  [GithubConnectEnd.Expired]:
    "The code expired before it was entered. Connect again for a new one.",
  [GithubConnectEnd.Denied]: "The sign-in was declined on GitHub.",
  [GithubConnectEnd.Cancelled]: "The sign-in was cancelled.",
  [GithubConnectEnd.Rejected]:
    "GitHub refused the Nessa app. Update Nessa and try again.",
  [GithubConnectEnd.Unreachable]:
    "Couldn't reach GitHub. Check the connection and try again.",
  [GithubConnectEnd.StoreUnavailable]: "The sign-in couldn't be saved. Connect again.",
  [GithubConnectEnd.AuditUnavailable]:
    "The sign-in couldn't be recorded, so it wasn't kept. Connect again.",
}

const unanswered = "The gateway didn't answer. Try again."

/** The model's view of one answer. */
export function viewOf(connection: GithubConnection): GithubView {
  switch (connection.state) {
    case "connected":
      return {
        kind: "connected",
        login: connection.account?.login ?? "",
        installUrl: connection.installUrl,
      }
    case "connecting":
      return {
        kind: "connecting",
        userCode: connection.code?.userCode ?? "",
        verificationUri: connection.code?.verificationUri ?? "",
        expiresInSeconds: connection.code?.expiresInSeconds ?? 0,
        installUrl: connection.installUrl,
      }
    case "signed_out":
      return {
        kind: "signedOut",
        ended: connection.last ? endedBecause[connection.last] : undefined,
        installUrl: connection.installUrl,
      }
  }
}

function failed(cause: unknown): GithubOutcome {
  const refusal = cause instanceof NessaGithubError ? cause.refusal : undefined
  return { ok: false, sentence: refusal ? refusals[refusal] : unanswered }
}

export function githubGateway(options: {
  connected: () => Promise<GithubClient>
  pollMs?: number
}): GithubGateway {
  const ask = async (
    call: (api: GithubApi) => Promise<GithubConnection>,
  ): Promise<GithubOutcome> => {
    try {
      const client = await options.connected()
      return { ok: true, view: viewOf(await call(client.github)) }
    } catch (cause) {
      return failed(cause)
    }
  }
  return {
    pollMs: options.pollMs ?? 2_000,
    read: () => ask((api) => api.status()),
    connect: () => ask((api) => api.connect()),
    cancel: () => ask((api) => api.cancel()),
    disconnect: () => ask((api) => api.disconnect()),
  }
}
