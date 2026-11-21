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
import { ChatgptSignInEnd, type LoginsStatus } from "@nessa/client"

/**
 * How a ChatGPT sign-in ended, in the window's words: `undefined` for one
 * that needs no sentence (the person cancelled it, or started another).
 * The one table both the card and Settings say it from.
 */
const endings: Record<ChatgptSignInEnd, string | undefined> = {
  [ChatgptSignInEnd.SignedIn]: "Signed in with ChatGPT.",
  [ChatgptSignInEnd.Declined]: "The sign-in was declined in the browser.",
  [ChatgptSignInEnd.Incomplete]: "The browser came back without a sign-in. Try again.",
  [ChatgptSignInEnd.WrongAccount]:
    "That was a different ChatGPT account. Sign in with the same one.",
  [ChatgptSignInEnd.ExchangeFailed]: "OpenAI didn’t finish the sign-in. Try again.",
  [ChatgptSignInEnd.InvalidIdentity]:
    "OpenAI’s answer couldn’t be checked, so it wasn’t kept.",
  [ChatgptSignInEnd.PlanNotEnabled]: "This ChatGPT plan can’t be used for Codex here.",
  [ChatgptSignInEnd.TooManyAccounts]:
    "Nessa keeps up to 8 ChatGPT accounts. Remove one in Settings first.",
  [ChatgptSignInEnd.NotSaved]: "Couldn’t keep the sign-in in Nessa. Try again.",
  [ChatgptSignInEnd.TimedOut]: "The sign-in took too long. Try again.",
  [ChatgptSignInEnd.Cancelled]: undefined,
  [ChatgptSignInEnd.Replaced]: undefined,
  [ChatgptSignInEnd.ListenFailed]:
    "Nessa couldn’t wait for the browser on this computer. Try again.",
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

/** Over the gateway's `logins` methods; `open` hands an address to the
 * person's browser where the host can, `pause` waits between reads. */
export function codexChatgptSignIn(ports: {
  status: () => Promise<LoginsStatus>
  begin: () => Promise<LoginsStatus>
  open?: (url: string) => void
  pause: (ms: number) => Promise<void>
  pollMs?: number
}): CodexChatgptSignIn {
  const pollMs = ports.pollMs ?? 2000
  return {
    available: async () => {
      try {
        return (await ports.status()).chatgpt !== undefined
      } catch {
        return false
      }
    },
    signIn: async (opened) => {
      let status: LoginsStatus
      try {
        status = await ports.begin()
      } catch (cause) {
        throw new ChatgptSignInFailure({ cause })
      }
      const started = status.chatgpt?.signIn
      if (started?.state !== "waiting" || !started.authorizeUrl || !started.attempt)
        return (
          ended(status, started?.attempt) ?? Promise.reject(new ChatgptSignInFailure())
        )
      const attempt = started.attempt
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
        let now: LoginsStatus
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
  status: LoginsStatus,
  attempt: string | undefined,
): ChatgptSignInEnd | undefined {
  const signIn = status.chatgpt?.signIn
  if (!signIn) return undefined
  if (signIn.attempt !== attempt)
    return signIn.state === "idle" && !signIn.attempt
      ? ChatgptSignInEnd.Cancelled
      : ChatgptSignInEnd.Replaced
  return signIn.state === "idle" ? (signIn.last ?? ChatgptSignInEnd.Cancelled) : undefined
}
