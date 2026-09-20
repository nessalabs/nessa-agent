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
    digest?: (bytes: Blob) => Promise<string>
  } = {},
) {
  const config = options.environment ?? loadEnvironment({})
  const session = createSessionHandle()
  return {
    session,
    attachments: createAttachmentResources(),
    // The Web Crypto digest an upload is identified by. Outside the process
    // like any other read, so it arrives here rather than being reached for:
    // tests hash with a function they wrote and never touch `crypto.subtle`.
    digest: options.digest ?? sha256Digest,
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
          )),
  }
}
export type AppDependencies = ReturnType<typeof createDependencies>
