import { readFileSync } from "node:fs"
import { spawnSync } from "node:child_process"
import { deployment, stageConfiguration } from "./deployment.mjs"
const [stage, ...extra] = process.argv.slice(2)
if (extra.length) throw new Error("deploy accepts one stage")
const paths = JSON.parse(
  readFileSync(new URL("../stage-deployments.json", import.meta.url), "utf8"),
)
const publicStages = JSON.parse(
  readFileSync(
    new URL("../../../protocol/defaults/github-relays.json", import.meta.url),
    "utf8",
  ),
)
stageConfiguration(stage, publicStages)
const targets = Object.fromEntries(
  Object.entries(paths).map(([name, path]) => [
    name,
    path
      ? JSON.parse(
          readFileSync(new URL(`../${path}`, import.meta.url), "utf8").replace(
            /,\s*([}\]])/g,
            "$1",
          ),
        )
      : null,
  ]),
)
deployment(stage, targets)
const result = spawnSync(
  "pnpm",
  ["exec", "wrangler", "deploy", "--config", paths[stage]],
  { stdio: "inherit" },
)
process.exit(result.status ?? 1)
