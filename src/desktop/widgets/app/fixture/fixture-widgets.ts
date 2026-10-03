/**
 * The fixture server's names, apart from its app (`fixture-plugin.ts`): what
 * the sample workspace's conversation names the fixture's call by, without
 * importing a plugin (ADR 326 › Boundaries: the workspace imports widgets'
 * hosts and no plugin), as `fixture/sample-widgets.ts` is for the sample.
 */
import { appWidget } from "../model/app-ref"

/** The fixture server's name, as the gateway would name it. */
export const fixtureServer = "nessa-fixture"

/** The UI resource its tool declares. */
export const fixtureResourceUri = "ui://nessa-fixture/app.html"

/** Its one call's identity: the execution and the tool call. */
export const fixtureCallIdentity = {
  executionId: "fixture-execution",
  toolId: "fixture-call",
} as const

/** The widget its one call is drawn as, in session `sessionId`. */
export function fixtureWidget(sessionId: string) {
  return appWidget(
    fixtureServer,
    sessionId,
    fixtureCallIdentity.executionId,
    fixtureCallIdentity.toolId,
  )
}
