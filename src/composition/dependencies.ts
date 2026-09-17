import { gatewayEffects } from "../conversation/adapters/gateway/effects"
import { createAttachmentResources } from "../panel/adapters/attachment-resources"
import { nativeCredentialSource } from "../session/adapters/client/credential-source"
import type { CredentialSource } from "@nessa/client"
import { loadEnvironment, type Environment } from "../env/environment"
import { scenarioEffects } from "../conversation/adapters/scenario/effects"
import { createSessionHandle } from "../session/adapters/client/handle"
import { connectDevSession } from "../session/adapters/client/dev-session"
import type { ConversationEffects } from "../conversation/application/ports"

/** Construct once per application. Overrides are explicit, never a service locator. */
export function createDependencies(
  options: {
    environment?: Environment
    connectSession?: typeof connectDevSession
    conversation?: ConversationEffects
    credentialSource?: CredentialSource
    clientId?: string
  } = {},
) {
  const config = options.environment ?? loadEnvironment({})
  const session = createSessionHandle()
  return {
    session,
    attachments: createAttachmentResources(),
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
        : gatewayEffects(() => session.get())),
  }
}
export type AppDependencies = ReturnType<typeof createDependencies>
