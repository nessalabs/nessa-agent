import { gatewayEffects } from "../conversation/adapters/gateway/effects"
import { createAttachmentResources } from "../panel/adapters/attachment-resources"
import { sha256Digest } from "../panel/adapters/sha256"
import { nativeCredentialSource } from "../session/adapters/client/credential-source"
import type { CredentialSource } from "@nessa/client"
import { loadEnvironment, type Environment } from "../env/environment"
import { scenarioEffects } from "../conversation/adapters/scenario/effects"
import { createSessionHandle } from "../session/adapters/client/handle"
import { connectDevSession } from "../session/adapters/client/dev-session"
import type { ConversationEffects } from "../conversation/application/ports"
import { httpAgentReadiness } from "../onboarding/adapters/agents"
import { loadChosenAgent } from "../host"
import type { AgentReadinessSource } from "../onboarding/application/ports"
import { hasNativeHost } from "../host"

/** Construct once per application. Overrides are explicit, never a service locator. */
export function createDependencies(
  options: {
    environment?: Environment
    connectSession?: typeof connectDevSession
    conversation?: ConversationEffects
    agents?: AgentReadinessSource
    credentialSource?: CredentialSource
    clientId?: string
    digest?: (bytes: Blob) => Promise<string>
    canChoosePaths?: boolean
  } = {},
) {
  const config = options.environment ?? loadEnvironment({})
  const session = createSessionHandle()
  // Asked once per conversation until somebody has chosen, and once per
  // application from then on. Only a choice is an answer; everything else is
  // asked again.
  //
  // Keeping a "nobody chose" looks safe and is not, because the panel exists
  // before the choice is written. The panel webview is up from startup and
  // setup is a second window beside it, whose third step teaches the summon
  // shortcut by having the user press it — which puts them in a live composer,
  // one step after picking an agent and before `finish_setup` records
  // anything. A message sent from there creates a conversation, the host
  // truthfully answers "nobody has chosen", and a remembered answer would send
  // every conversation for the rest of the launch to the gateway's default. It
  // would look exactly like the choice having been honoured. The same holds for
  // a `settings.json` that could not be read, which `SettingsStore::load` turns
  // into defaults rather than an error.
  //
  // A failure is never kept either, in either shape it takes. A remembered
  // rejection would be a panel that can no longer start, send, close or answer
  // a permission until it is restarted.
  //
  // The cost of asking again is one local host round trip per `create`, on a
  // path that already awaits the gateway, and only until somebody has chosen.
  // Creations in the same tick share one call: the assignment below happens
  // synchronously and the clearing only in a later microtask.
  //
  // What this does not fix, and cannot from here: a conversation created
  // during that window is on record with the gateway's default for the rest of
  // its life, because the server reopens an existing conversation on the agent
  // its own record names and nothing in the panel renames one. The choice takes
  // effect from the next conversation.
  let chosen: Promise<string | undefined> | undefined
  const ask = (): Promise<string | undefined> => {
    // Each ask clears only what it put there. Without the identity check an
    // answer still in flight when a choice is handed over in place would erase
    // that choice on landing, and the surface that has no host to ask again is
    // exactly the one that hands it over.
    const asking: Promise<string | undefined> = loadChosenAgent()
      .then((answer) => {
        if (answer.outcome !== "chosen" && chosen === asking) chosen = undefined
        return answer.outcome === "chosen" ? answer.agent : undefined
      })
      // Unreachable through `loadChosenAgent`, which answers rather than
      // rejecting. Kept because the cost of being wrong about that is a memo
      // holding a rejected promise, which is a panel that cannot start, send,
      // close or answer a permission until it is restarted.
      .catch((cause: unknown) => {
        if (chosen === asking) chosen = undefined
        throw cause
      })
    return asking
  }
  const chosenAgent = () => (chosen ??= ask())
  return {
    session,
    attachments: createAttachmentResources(),
    /**
     * Keep a choice made on this surface, for a surface that has no host.
     *
     * In a browser setup hands over to the panel in place, in this same page:
     * there is nothing to write the choice to and nothing to read it back
     * from, so being told is the only record it gets. Without this the picker
     * is a control whose selection goes nowhere — the user is shown Codex as
     * ready, picks it, and every conversation runs on the gateway's default
     * with nothing on screen saying so.
     *
     * Held here rather than in the surface because this is where the same
     * question is answered for the desktop, and the agent a creation names has
     * to be one fact however the surface learned it.
     */
    rememberChosenAgent: (agent: string) => {
      chosen = Promise.resolve(agent)
    },
    // The Web Crypto digest an upload is identified by. Outside the process
    // like any other read, so it arrives here rather than being reached for:
    // tests hash with a function they wrote and never touch `crypto.subtle`.
    digest: options.digest ?? sha256Digest,
    // Whether this surface has a picker that can say where a file is. Only the
    // desktop host's does; a browser's file input hands over bytes and never
    // their location. The conversation vertical needs it to say why a file
    // cannot be sent, and is not allowed to ask the host itself — so it is
    // answered once here and injected, like every other outside fact.
    canChoosePaths: options.canChoosePaths ?? hasNativeHost(),
    // Setup's one pre-session question. Constructed here so the surface takes
    // it as a dependency rather than importing the transport it happens to use.
    agents: options.agents ?? httpAgentReadiness({ baseUrl: config.gatewayBaseUrl }),
    usesLocalSession: config.conversation.backend === "local",
    connectSession:
      options.connectSession ??
      (() =>
        connectDevSession({
          stage: config.stage,
          clientId: options.clientId,
          credentialSource: options.credentialSource ?? nativeCredentialSource(),
        })),
    conversation:
      options.conversation ??
      (config.conversation.backend === "scenario"
        ? scenarioEffects(config.conversation.scenario)
        : gatewayEffects(
            () => session.get(),
            // The real clock for backing off a busy upload route; tests pass theirs.
            (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
            chosenAgent,
          )),
  }
}
export type AppDependencies = ReturnType<typeof createDependencies>
