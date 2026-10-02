/** Real setup controller, UI and HTTP adapter; native gateway is unmanaged. */
import * as React from "react"
import { createRoot } from "react-dom/client"
import { httpAgentReadiness } from "../../../../src/onboarding/adapters/agents"
import {
  beginOnboarding,
  startAgentChoice,
} from "../../../../src/onboarding/model/onboarding"
import { Onboarding } from "../../../../src/onboarding/ui/onboarding"
import { useOnboarding } from "../../../../src/onboarding/ui/use-onboarding"
import "../../../../src/styles.css"

const params = new URL(location.href).searchParams
const token = params.get("token") ?? "standalone"
const phase = params.get("phase") === "body" ? "body" : "fetch"
const requests: { started: number; aborted: boolean }[] = []
let checkedAt: number | undefined
const source = httpAgentReadiness({
  baseUrl: `/readiness-probe/${encodeURIComponent(token)}/${phase}`,
  fetch: (input, init) => {
    const record = { started: performance.now(), aborted: false }
    requests.push(record)
    init?.signal?.addEventListener(
      "abort",
      () => {
        record.aborted = true
      },
      { once: true },
    )
    return fetch(input, init)
  },
})
function Surface() {
  const onboarding = useOnboarding(source, startAgentChoice(beginOnboarding()))
  React.useEffect(() => {
    if (!onboarding.checking && onboarding.state.readinessFailure)
      checkedAt = performance.now()
  }, [onboarding.checking, onboarding.state.readinessFailure])
  return (
    <>
      <output
        data-readiness
      >{`${onboarding.checking}:${onboarding.state.readiness?.claude ?? "unknown"}`}</output>
      <Onboarding
        state={onboarding.state}
        gatewayStartup={onboarding.gatewayStartup}
        checking={onboarding.checking}
        platform={onboarding.platform}
        onBegin={onboarding.begin}
        onChoose={onboarding.choose}
        onConfirm={onboarding.confirm}
        onFinish={onboarding.finish}
        onRecheck={onboarding.recheck}
        onRetryGateway={onboarding.retryGatewayStartup}
      />
    </>
  )
}
Object.assign(window, { __readinessProbe: { snapshot: () => ({ requests, checkedAt }) } })
const element = document.getElementById("root")
if (!element) throw new Error("The fixture root is absent")
createRoot(element).render(<Surface />)
