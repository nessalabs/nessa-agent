/** Real setup UI and controller. The linger port is the phase in the query. */
import * as React from "react"
import { createRoot } from "react-dom/client"

import type { LingerSource } from "../../../../src/onboarding/adapters/linger"
import type { LingerView } from "../../../../src/onboarding/model/linger"
import {
  beginOnboarding,
  chooseAgent,
  confirmAgent,
  recordReadiness,
  startAgentChoice,
} from "../../../../src/onboarding/model/onboarding"
import { Onboarding } from "../../../../src/onboarding/ui/onboarding"
import { useOnboarding } from "../../../../src/onboarding/ui/use-onboarding"
import "../../../../src/styles.css"

const offer: LingerView = { shown: "offer", audit: "not-required" }
const enabled: LingerView = { shown: "enabled", audit: "recorded" }
const declined: LingerView = { shown: "declined", audit: "recorded" }
const refused: LingerView = { shown: "refused", audit: "recorded" }
const unsupported: LingerView = { shown: "unsupported", audit: "not-required" }

const phase = new URL(location.href).searchParams.get("phase") ?? "offer-then-enable"
const probe = { accepts: 0, declines: 0 }

function source(): LingerSource {
  if (phase === "already-enabled") {
    return {
      status: async () => enabled,
      accept: async () => {
        probe.accepts += 1
        return enabled
      },
      decline: async () => declined,
    }
  }
  if (phase === "unsupported") {
    return {
      status: async () => unsupported,
      accept: async () => {
        probe.accepts += 1
        return unsupported
      },
      decline: async () => declined,
    }
  }
  if (phase === "offer-then-decline") {
    return {
      status: async () => offer,
      accept: async () => {
        probe.accepts += 1
        return enabled
      },
      decline: async () => {
        probe.declines += 1
        return declined
      },
    }
  }
  if (phase === "refused") {
    return {
      status: async () => offer,
      accept: async () => {
        probe.accepts += 1
        return refused
      },
      decline: async () => declined,
    }
  }
  if (phase === "waiting") {
    return {
      status: async () => offer,
      accept: () =>
        new Promise(() => {
          probe.accepts += 1
        }),
      decline: async () => declined,
    }
  }
  return {
    status: async () => offer,
    accept: async () => {
      probe.accepts += 1
      return enabled
    },
    decline: async () => {
      probe.declines += 1
      return declined
    },
  }
}

const atSummon = confirmAgent(
  chooseAgent(
    startAgentChoice(recordReadiness(beginOnboarding(), { claude: "ready" })),
    "claude",
  ),
)
const agents = { read: async () => ({ ok: true as const, agents: {} }) }

function Surface() {
  const linger = React.useMemo(source, [])
  const onboarding = useOnboarding(agents, atSummon, linger)
  return (
    <div data-step={onboarding.state.step}>
      <Onboarding
        state={onboarding.state}
        gatewayStartup={onboarding.gatewayStartup}
        platform="linux"
        accelerator={onboarding.accelerator}
        lingerPending={onboarding.lingerPending}
        onBegin={onboarding.begin}
        onChoose={onboarding.choose}
        onConfirm={onboarding.confirm}
        onFinish={onboarding.finish}
        onAcceptLinger={onboarding.acceptLinger}
        onDeclineLinger={onboarding.declineLinger}
        onRecheck={onboarding.recheck}
        onRetryGateway={onboarding.retryGatewayStartup}
      />
    </div>
  )
}

Object.assign(window, { __lingerProbe: probe })
const element = document.getElementById("root")
if (!element) throw new Error("The fixture root is absent")
createRoot(element).render(<Surface />)
