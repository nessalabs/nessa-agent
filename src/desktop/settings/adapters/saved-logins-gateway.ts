/**
 * The gateway's saved logins (`client.logins`) in the window's words, for
 * Settings › Accounts and a computer's page in Devices. Each refusal is a
 * sentence from a total table, so a code the protocol adds is a type error
 * here rather than a sentence nobody wrote. A login goes in and never comes
 * back: what is read is only which agents have one, and which computers are
 * switched on to be handed them.
 *
 * Codex's ChatGPT accounts come with them (`status.chatgpt`), when the
 * gateway keeps them: each account's email and whether it is in use or needs
 * signing in again, and where a sign-in stands. A token never comes back.
 */
import {
  LoginsRefusalCode,
  NessaLoginsError,
  type ChatgptAccount,
  type ChatgptSignInEnd,
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
  [LoginsRefusalCode.LoginsChangeUnrecorded]:
    "Changed, but Nessa couldn’t keep a record of it.",
  [LoginsRefusalCode.LoginsTooManyDevices]:
    "Too many computers use your saved logins. Turn one off first.",
  [LoginsRefusalCode.LoginsUnknownAccount]:
    "That ChatGPT account isn’t kept any more. Look again.",
  [LoginsRefusalCode.LoginsChatgptNotConfigured]:
    "This gateway can’t sign in with ChatGPT right now.",
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

export { chatgptEndSentence } from "../../../provider-authentication/model/chatgpt-sign-in"

/** Where a ChatGPT sign-in stands. */
export type ChatgptSignInView =
  | { readonly kind: "idle"; readonly last?: ChatgptSignInEnd }
  | {
      readonly kind: "waiting"
      /** Where the browser is sent, on the gateway's own computer. */
      readonly authorizeUrl: string
      /** The account signing in again; absent for a new one. */
      readonly accountId?: string
    }

/** Codex's ChatGPT accounts, and the sign-in. */
export interface ChatgptView {
  readonly accounts: readonly ChatgptAccount[]
  readonly signIn: ChatgptSignInView
}

/** Which agents have a login saved. */
export interface SavedLogins {
  readonly saved: Readonly<Record<LoginAgent, boolean>>
  /** Codex's ChatGPT accounts; absent when the gateway keeps none. */
  readonly chatgpt?: ChatgptView
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
  /** Start a ChatGPT sign-in: of `accountId` again, or of a new account. */
  chatgptSignIn(accountId?: string): Promise<SavedLoginsOutcome>
  chatgptCancel(): Promise<SavedLoginsOutcome>
  /** Have Codex use `accountId`, or its API key when `null`. */
  chatgptUse(accountId: string | null): Promise<SavedLoginsOutcome>
  chatgptRemove(accountId: string): Promise<SavedLoginsOutcome>
}

function read(status: LoginsStatus): SavedLogins {
  const saved = { claude: false, codex: false, opencode: false }
  for (const login of status.logins) saved[login.agent] = login.saved
  const logins = { saved }
  const chatgpt = status.chatgpt
  if (!chatgpt) return logins
  const { signIn } = chatgpt
  return {
    ...logins,
    chatgpt: {
      accounts: chatgpt.accounts,
      signIn:
        signIn.state === "waiting" && signIn.authorizeUrl
          ? {
              kind: "waiting",
              authorizeUrl: signIn.authorizeUrl,
              ...(signIn.accountId ? { accountId: signIn.accountId } : {}),
            }
          : { kind: "idle", ...(signIn.last ? { last: signIn.last } : {}) },
    },
  }
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
    chatgptSignIn: (accountId) => ask((logins) => logins.chatgptSignIn(accountId)),
    chatgptCancel: () => ask((logins) => logins.chatgptCancel()),
    chatgptUse: (accountId) => ask((logins) => logins.chatgptUse(accountId)),
    chatgptRemove: (accountId) => ask((logins) => logins.chatgptRemove(accountId)),
  }
}
