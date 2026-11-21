/**
 * Codex on another computer (an SSH host, a paired computer, a Boat or
 * Agent37 VM) can run on a ChatGPT plan instead of an API key: the gateway
 * signs the account in and hands that computer short-lived tokens
 * (`docs/design/chatgpt-sign-in.md`). Its sign-in card starts that sign-in,
 * opens the browser on it, and waits for how it ends; Settings › Saved
 * logins does the same, and lists the accounts.
 *
 * ```text
 * Continue with ChatGPT ──▶ chatgptSignIn ──▶ waiting {authorizeUrl}
 *   ──▶ the browser opens it (or the card links to it)
 *   ──▶ status read every pollMs ──▶ idle {last} ──▶ the card says how it ended
 * ```
 */

/** How a ChatGPT sign-in ended, as the gateway names it. */
export type ChatgptSignInEnd =
  | "signed_in"
  | "declined"
  | "incomplete"
  | "wrong_account"
  | "exchange_failed"
  | "invalid_identity"
  | "plan_not_enabled"
  | "too_many_accounts"
  | "not_saved"
  | "timed_out"
  | "cancelled"
  | "replaced"
  | "listen_failed"

/** What the card reads of the gateway's ChatGPT accounts and sign-in. */
export interface ChatgptSignInState {
  readonly accounts: readonly {
    readonly id: string
    readonly inUse: boolean
    readonly needsSignIn: boolean
  }[]
  readonly signIn: {
    readonly state: "idle" | "waiting"
    readonly attempt?: string
    readonly authorizeUrl?: string
    readonly last?: ChatgptSignInEnd
  }
  /** Only in the answer to starting one: the attempt that call started. */
  readonly started?: string
}

/**
 * How a ChatGPT sign-in ended, in the window's words: `undefined` for one
 * that needs no sentence (the person cancelled it, or started another).
 * The one table both the card and Settings say it from.
 */
const endings: Record<ChatgptSignInEnd, string | undefined> = {
  signed_in: "Signed in with ChatGPT.",
  declined: "The sign-in was declined in the browser.",
  incomplete: "The browser came back without a sign-in. Try again.",
  wrong_account: "That was a different ChatGPT account. Sign in with the same one.",
  exchange_failed: "OpenAI didn’t finish the sign-in. Try again.",
  invalid_identity: "OpenAI’s answer couldn’t be checked, so it wasn’t kept.",
  plan_not_enabled: "This ChatGPT plan can’t be used for Codex here.",
  too_many_accounts:
    "Nessa keeps up to 8 ChatGPT accounts. Remove one in Settings first.",
  not_saved: "Couldn’t keep the sign-in in Nessa. Try again.",
  timed_out: "The sign-in took too long. Try again.",
  cancelled: undefined,
  replaced: undefined,
  listen_failed: "Nessa couldn’t wait for the browser on this computer. Try again.",
}

/** The sentence for how a ChatGPT sign-in ended, if it needs one. */
export function chatgptEndSentence(end: ChatgptSignInEnd): string | undefined {
  return endings[end]
}

/** Why a card's ChatGPT sign-in gave no ending: it could not start, or the
 * gateway stopped answering while it waited. */
export class ChatgptSignInFailure extends Error {
  constructor(options?: { cause?: unknown }) {
    super("ChatGPT sign-in did not complete", options)
    this.name = "ChatGPTSignInFailure"
  }
}

/** A ChatGPT sign-in for Codex, as the card runs it. */
export interface CodexChatgptSignIn {
  /** Whether the gateway keeps ChatGPT accounts. */
  available(): Promise<boolean>
  /**
   * Start one, open the browser on it with `opened` told the address, and
   * wait for how it ends. Rejects with {@link ChatgptSignInFailure}.
   */
  signIn(opened: (authorizeUrl: string) => void): Promise<ChatgptSignInEnd>
}

/** Over the gateway's `logins` methods, each answering its ChatGPT part
 * (`undefined` where the gateway keeps no accounts); `open` hands an
 * address to the person's browser where the host can, `pause` waits
 * between reads. `begin` signs in again the account named, or a new one. */
export function codexChatgptSignIn(ports: {
  status: () => Promise<ChatgptSignInState | undefined>
  begin: (accountId?: string) => Promise<ChatgptSignInState | undefined>
  open?: (url: string) => void
  pause: (ms: number) => Promise<void>
  pollMs?: number
}): CodexChatgptSignIn {
  const pollMs = ports.pollMs ?? 2000
  return {
    available: async () => {
      try {
        return (await ports.status()) !== undefined
      } catch {
        return false
      }
    },
    signIn: async (opened) => {
      let status: ChatgptSignInState | undefined
      try {
        // The account in use signs in again when it needs to, so the card
        // renews what Codex uses rather than adding an account beside it.
        const before = await ports.status()
        const again = before?.accounts.find(
          (account) => account.inUse && account.needsSignIn,
        )
        status = await ports.begin(again?.id)
      } catch (cause) {
        throw new ChatgptSignInFailure({ cause })
      }
      // The answer names the attempt this call started; the sign-in it
      // shows may already be another window's.
      const attempt = status?.started
      const started = status?.signIn
      if (!attempt) throw new ChatgptSignInFailure()
      if (
        started?.state !== "waiting" ||
        started.attempt !== attempt ||
        !started.authorizeUrl
      )
        return ended(status, attempt) ?? Promise.reject(new ChatgptSignInFailure())
      opened(started.authorizeUrl)
      try {
        ports.open?.(started.authorizeUrl)
      } catch {
        // The card still links to it.
      }
      // The gateway ends a sign-in by itself after ten minutes; a little
      // past that, the card stops waiting too.
      for (let waited = 0; waited <= 11 * 60_000; waited += pollMs) {
        await ports.pause(pollMs)
        let now: ChatgptSignInState | undefined
        try {
          now = await ports.status()
        } catch (cause) {
          throw new ChatgptSignInFailure({ cause })
        }
        const end = ended(now, attempt)
        if (end) return end
      }
      throw new ChatgptSignInFailure()
    },
  }
}

/**
 * How `attempt` ended, once it has: its own end, or replaced when another
 * attempt (another window's, say) is the one waiting or last ended now, so
 * the card never reports someone else's sign-in as its own.
 */
function ended(
  status: ChatgptSignInState | undefined,
  attempt: string | undefined,
): ChatgptSignInEnd | undefined {
  const signIn = status?.signIn
  if (!signIn) return undefined
  if (signIn.attempt !== attempt)
    return signIn.state === "idle" && !signIn.attempt ? "cancelled" : "replaced"
  return signIn.state === "idle" ? (signIn.last ?? "cancelled") : undefined
}
