/** The manual desktop job's scope and evidence identity have one owner. */
import { pathToFileURL } from "node:url"

const plans = Object.freeze({
  full: Object.freeze({
    scope: "full",
    check: "",
    buildGateway: true,
    artifactPrefix: "desktop-browser-qualification",
  }),
  "frame-budget": Object.freeze({
    scope: "frame-budget",
    check: "perf-budget",
    buildGateway: false,
    artifactPrefix: "desktop-frame-budget",
  }),
  "mcp-apps": Object.freeze({
    scope: "mcp-apps",
    check: "mcp-apps",
    buildGateway: false,
    artifactPrefix: "desktop-mcp-apps",
  }),
})

export class DesktopMeasurementScopeError extends Error {
  constructor() {
    super(`Expected a desktop measurement scope: ${Object.keys(plans).join(", ")}.`)
    this.name = "DesktopMeasurementScopeError"
  }
}

export function desktopMeasurementPlan(scope) {
  if (typeof scope !== "string" || !Object.hasOwn(plans, scope))
    throw new DesktopMeasurementScopeError()
  return plans[scope]
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const args = process.argv.slice(2)
    if (args.length !== 1) throw new DesktopMeasurementScopeError()
    const plan = desktopMeasurementPlan(args[0])
    process.stdout.write(
      `scope=${plan.scope}\ncheck=${plan.check}\nbuild-gateway=${plan.buildGateway}\nartifact-prefix=${plan.artifactPrefix}\n`,
    )
  } catch (error) {
    if (!(error instanceof DesktopMeasurementScopeError)) throw error
    process.stderr.write(`${error.message}\n`)
    process.exitCode = 2
  }
}
