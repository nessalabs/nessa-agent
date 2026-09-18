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
  // What is never kept is a failure. Every conversation is created through this,
  // so a remembered rejection is not one lost answer, it is a panel that can no
  // longer start, send, close or answer a permission until it is restarted.
  let chosen: Promise<string | undefined> | undefined
  const chosenAgent = () =>
    (chosen ??= loadChosenAgent().catch((cause: unknown) => {
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
