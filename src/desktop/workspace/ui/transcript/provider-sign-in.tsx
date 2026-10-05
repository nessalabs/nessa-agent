import { ProviderSignInCard } from "../../../../provider-authentication/ui/provider-sign-in"
import { useCallback } from "react"
import { providerLoginAvailable, signInToProvider } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"

/** The workspace supplies the shared card's external login action. */
export function ProviderSignIn({ provider }: { provider: "claude" | "codex" }) {
  const dispatch = useWorkspaceDispatch()
  const canSignIn = useCallback(() => dispatch(providerLoginAvailable()), [dispatch])
  const signIn = useCallback(
    (chosen: "claude" | "codex") => dispatch(signInToProvider(chosen)),
    [dispatch],
  )
  return (
    <ProviderSignInCard provider={provider} onSignIn={signIn} canSignIn={canSignIn} />
  )
}
