import { ProviderSignInCard } from "../../../../provider-authentication/ui/provider-sign-in"
import {
  signInAction,
  type LeaseEnvironment,
} from "../../../../provider-authentication/model/boat-sign-in"
import { useCallback } from "react"
import {
  boatSignInAvailable,
  providerLoginAvailable,
  signInForBoat,
  signInToProvider,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"

/** The workspace supplies the shared card's sign-in: this computer's
 * external login, or a sign-in minted for Boat where the refused run was. */
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
  return (
    <ProviderSignInCard
      provider={provider}
      action={action}
      onSignIn={signIn}
      canSignIn={canSignIn}
    />
  )
}
