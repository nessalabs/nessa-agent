/**
 * The window over a fake gateway holding one conversation with an armed
 * review. `approval-deadline.mjs` drives it.
 *
 * The review is due a few seconds after the page loads. The card counts down
 * from that instant. When the wait ends, the fixture publishes the notice the
 * deadline would leave and drops the review, which is what the gateway does
 * when the deadline wins. The desktop does not decide the review itself.
 *
 * The fake gateway's clock is a minute ahead of the page's, as a gateway on
 * another machine can be: the card still counts down from the wait, because
 * it measures `dueAtMs` against the gateway's own `serverNowMs` (ADR 141).
 */
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { createDesktopDependencies } from "../../../../src/desktop/dependencies"
import { makeDesktopStore } from "../../../../src/desktop/store"
import { DesktopIconFamilyProvider } from "../../../../src/desktop/ui/icons"
import { DesktopWindow } from "../../../../src/desktop/ui/desktop-window"
import { WidgetRegistryProvider } from "../../../../src/desktop/widgets"
import { ClockProvider } from "../../../../src/clock"
import { followWorkspace, loadWorkspace } from "../../../../src/desktop/workspace"
import {
  fakeGateway,
  row,
} from "../../../../src/desktop/workspace/adapters/gateway/fake-gateway"
import {
  gatewaySource,
  type GatewayClock,
} from "../../../../src/desktop/workspace/adapters/gateway/gateway-source"

import { armed, conversation, shown, waitMs } from "./state"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "../../../../src/desktop/styles.css"

/** The gateway's clock, a minute ahead of the page's. */
const gatewayNow = () => Date.now() + 60_000
const dueAtMs = gatewayNow() + waitMs

const gateway = fakeGateway({ now: gatewayNow })
let revision = 1
gateway.rows.set(
  conversation,
  row(conversation, { title: "Armed review", running: true }),
)
gateway.views.set(conversation, shown(revision, armed(dueAtMs)))

// The page's wall clock, the one the card's countdown is read against
// (`createDesktopDependencies`' default `now`).
const clock: GatewayClock = {
  now: () => Date.now(),
  after(ms, run) {
    const timer = window.setTimeout(run, ms)
    return () => window.clearTimeout(timer)
  },
}
const source = gatewaySource({
  connect: () => Promise.resolve(gateway.client),
  clock,
})
const dependencies = createDesktopDependencies({ workspace: source })
const store = makeDesktopStore(dependencies)
store.dispatch(followWorkspace())
void store.dispatch(loadWorkspace())

window.setTimeout(() => {
  gateway.rows.set(
    conversation,
    row(conversation, { title: "Armed review", running: false }),
  )
  gateway.views.set(conversation, shown(++revision, undefined))
  gateway.publish(conversation)
  gateway.publishList()
}, waitMs)

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")
createRoot(container).render(
  <React.StrictMode>
    <Provider store={store}>
      <WidgetRegistryProvider registry={dependencies.widgets}>
        <ClockProvider now={dependencies.now}>
          <DesktopIconFamilyProvider>
            <DesktopWindow hostKind="browser" browserSurface inspectable={false} />
          </DesktopIconFamilyProvider>
        </ClockProvider>
      </WidgetRegistryProvider>
    </Provider>
  </React.StrictMode>,
)
