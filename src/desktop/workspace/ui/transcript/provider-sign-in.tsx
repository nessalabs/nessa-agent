import { ProviderSignInCard } from "../../../../provider-authentication/ui/provider-sign-in"
import { signInToProvider } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"

/** The workspace supplies the shared card's external login action. */
export function ProviderSignIn({ provider }: { provider: "claude" | "codex" }) {
  const dispatch = useWorkspaceDispatch()
  return (
    <ProviderSignInCard
      provider={provider}
      onSignIn={(chosen) => dispatch(signInToProvider(chosen))}
    />
  )
}
