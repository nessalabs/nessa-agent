/** Real setup agent list. Progress is local state, not a staged runtime. */
import * as React from "react"
import { createRoot } from "react-dom/client"

import type { RuntimeCopy } from "../../../../src/host"
import {
  beginOnboarding,
  recordReadiness,
  startAgentChoice,
} from "../../../../src/onboarding/model/onboarding"
import { Onboarding } from "../../../../src/onboarding/ui/onboarding"
import "../../../../src/styles.css"

const state = recordReadiness(startAgentChoice(beginOnboarding()), { claude: "ready" })
const copying: RuntimeCopy = {
  state: "copying",
  filesCompleted: 0,
  filesTotal: 4,
  bytesCompleted: 0,
  bytesTotal: 40,
}
const halfway: RuntimeCopy = { ...copying, filesCompleted: 2, bytesCompleted: 20 }
const settled: RuntimeCopy = {
  state: "settled",
  filesCompleted: 0,
  filesTotal: 0,
  bytesCompleted: 0,
  bytesTotal: 0,
}

function Fixture() {
  const [progress, setProgress] = React.useState<RuntimeCopy | undefined>(copying)
  return (
    <>
      <Onboarding
        state={state}
        gatewayStartup={{ revision: 1, state: "ready" }}
        platform="apple"
        runtimeCopy={progress}
        onBegin={() => {}}
        onChoose={() => {}}
        onConfirm={() => {}}
        onFinish={() => {}}
        onRecheck={() => {}}
        onRetryGateway={() => {}}
      />
      <button type="button" onClick={() => setProgress(halfway)}>
        Advance runtime copy
      </button>
      <button type="button" onClick={() => setProgress(settled)}>
        Finish runtime copy
      </button>
    </>
  )
}

const root = document.getElementById("root")
if (!root) throw new Error("Missing root")
createRoot(root).render(<Fixture />)
