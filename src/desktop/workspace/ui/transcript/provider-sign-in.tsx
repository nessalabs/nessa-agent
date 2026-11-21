import {
  ChatgptSignInCard,
  ProviderSignInCard,
} from "../../../../provider-authentication/ui/provider-sign-in"
import type { CodexChatgptSignIn } from "../../../../provider-authentication/model/chatgpt-sign-in"
import {
  signInAction,
  type LeaseEnvironment,
} from "../../../../provider-authentication/model/boat-sign-in"
import { useCallback, useMemo } from "react"
import {
  boatSignInAvailable,
  chatgptSignInAvailable,
  providerLoginAvailable,
  signInWithChatgpt,
  signInForBoat,
  signInToProvider,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"

/** The workspace supplies the shared card's sign-in: this computer's
 * external login, a sign-in minted for Boat where the refused run was, or a
 * ChatGPT account for Codex on another computer. */
export function ProviderSignIn({
  provider,
  environment,
}: {
  provider: "claude" | "codex"
  environment?: LeaseEnvironment
}) {
  const dispatch = useWorkspaceDispatch()
  const action = signInAction(provider, environment)
  const canSignIn = useCallback(
    () => dispatch(action === "boat" ? boatSignInAvailable() : providerLoginAvailable()),
    [dispatch, action],
  )
  const signIn = useCallback(
    (chosen: "claude" | "codex") =>
      dispatch(action === "boat" ? signInForBoat() : signInToProvider(chosen)),
    [dispatch, action],
  )
  const chatgpt = useMemo<CodexChatgptSignIn>(
    () => ({
      available: () => dispatch(chatgptSignInAvailable()),
      signIn: (opened) => dispatch(signInWithChatgpt(opened)),
    }),
    [dispatch],
  )
  if (action === "chatgpt") return <ChatgptSignInCard signIn={chatgpt} />
  return (
    <ProviderSignInCard
      provider={provider}
      action={action}
      onSignIn={signIn}
      canSignIn={canSignIn}
    />
  )
}
