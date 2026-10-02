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

/** The execution and tool call ids of its one call. */
export const fixtureExecutionId = "fixture-execution"
export const fixtureToolId = "fixture-call"

/** The widget its one call is drawn as. */
export const fixtureWidget = appWidget(fixtureServer, fixtureExecutionId, fixtureToolId)
