import { gatewayEffects } from "../conversation/adapters/gateway/effects"
import { createAttachmentResources } from "../panel/adapters/attachment-resources"
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

/** Construct once per application. Overrides are explicit, never a service locator. */
export function createDependencies(
  options: {
    environment?: Environment
    connectSession?: typeof connectDevSession
    conversation?: ConversationEffects
    agents?: AgentReadinessSource
    credentialSource?: CredentialSource
    clientId?: string
  } = {},
) {
  const config = options.environment ?? loadEnvironment({})
  const session = createSessionHandle()
  // Asked once per application rather than once per conversation. The answer is
  // written down before the panel exists and nothing changes it while the panel
  // runs, so re-asking would be one host round trip per new conversation for an
  // answer that cannot have moved.
  //
  // What is never kept is a failure, in either of the two shapes it takes. A
  // remembered rejection would be a panel that can no longer start, send, close
  // or answer a permission until it is restarted. A remembered "the host could
  // not be asked" is quieter and lasts just as long: it reads as "nobody chose",
  // so one unlucky round trip during startup sends every conversation for the
  // rest of the panel's life to the gateway's default — and the agent the user
  // picked is never asked for again, however well the host recovers.
  let chosen: Promise<string | undefined> | undefined
  const chosenAgent = () =>
    (chosen ??= loadChosenAgent()
      .then((answer) => {
        if (answer.outcome === "unavailable") chosen = undefined
        return answer.outcome === "chosen" ? answer.agent : undefined
      })
      .catch((cause: unknown) => {
        chosen = undefined
        throw cause
      }))
  return {
    session,
    attachments: createAttachmentResources(),
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
        : gatewayEffects(() => session.get(), chosenAgent)),
  }
}
export type AppDependencies = ReturnType<typeof createDependencies>
