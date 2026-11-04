/**
 * The gateway's saved logins (`client.logins`) in the window's words, for
 * Settings › Accounts and a computer's page in Devices. Each refusal is a
 * sentence from a total table, so a code the protocol adds is a type error
 * here rather than a sentence nobody wrote. A login goes in and never comes
 * back: what is read is only which agents have one, and which computers are
 * switched on to be handed them.
 */
import {
  LoginsRefusalCode,
  NessaLoginsError,
  type LoginAgent,
  type LoginsApi,
  type LoginsRefusalCode as LoginsRefusal,
  type LoginsStatus,
} from "@nessa/client"

/** What the saved-logins adapter asks of the window's gateway client. */
export interface SavedLoginsClient {
  readonly logins: LoginsApi
}

const refusals: Record<LoginsRefusal, string> = {
  [LoginsRefusalCode.LoginsNotConfigured]: "This gateway doesn’t keep saved logins.",
  [LoginsRefusalCode.LoginsStorageUnavailable]:
    "Couldn’t read or change the saved logins. Try again.",
  [LoginsRefusalCode.LoginsAuditUnavailable]:
    "Couldn’t keep a record of this, so nothing changed. Try again.",
  [LoginsRefusalCode.LoginsTooManyDevices]:
    "Too many computers use your saved logins. Turn one off first.",
  [LoginsRefusalCode.VaultLocked]: "Unlock Nessa to change saved logins.",
  [LoginsRefusalCode.Forbidden]: "This window isn’t allowed to change saved logins.",
  [LoginsRefusalCode.Unauthorized]: "This window isn’t allowed to change saved logins.",
  [LoginsRefusalCode.InvalidRequest]:
    "That doesn’t look like a login. Paste it again without spaces.",
  [LoginsRefusalCode.TemporarilyUnavailable]: "The gateway is busy. Try again.",
}

const unanswered = "The gateway didn’t answer. Try again."

/** The sentence for a logins method that did not answer with a result. */
export function savedLoginsRefusalSentence(error: unknown): string {
  return error instanceof NessaLoginsError && error.refusal
    ? refusals[error.refusal]
    : unanswered
}

/** Which agents have a login saved, and which computers receive them. */
export interface SavedLogins {
  readonly saved: Readonly<Record<LoginAgent, boolean>>
  readonly devicesOn: ReadonlySet<string>
}

/** The saved logins, or why the gateway would not say. */
export type SavedLoginsOutcome =
  | { readonly kind: "read"; readonly logins: SavedLogins }
  | { readonly kind: "refused"; readonly sentence: string }

/** The saved logins, as Settings and Devices manage them. */
export interface SavedLoginsGateway {
  read(): Promise<SavedLoginsOutcome>
  save(agent: LoginAgent, login: string): Promise<SavedLoginsOutcome>
  remove(agent: LoginAgent): Promise<SavedLoginsOutcome>
  setDevice(environment: string, on: boolean): Promise<SavedLoginsOutcome>
}

function read(status: LoginsStatus): SavedLogins {
  const saved = { claude: false, codex: false, opencode: false }
  for (const login of status.logins) saved[login.agent] = login.saved
  return { saved, devicesOn: new Set(status.devicesOn) }
}

export function savedLoginsGateway(
  connected: () => Promise<SavedLoginsClient>,
): SavedLoginsGateway {
  const ask = async (
    run: (logins: LoginsApi) => Promise<LoginsStatus>,
  ): Promise<SavedLoginsOutcome> => {
    try {
      return { kind: "read", logins: read(await run((await connected()).logins)) }
    } catch (error) {
      return { kind: "refused", sentence: savedLoginsRefusalSentence(error) }
    }
  }
  return {
    read: () => ask((logins) => logins.status()),
    save: (agent, login) => ask((logins) => logins.save(agent, login)),
    remove: (agent) => ask((logins) => logins.remove(agent)),
    setDevice: (environment, on) => ask((logins) => logins.setDevice(environment, on)),
  }
}
