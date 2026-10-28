import { describe, expect, it } from "vitest"

import timing from "../../../../host/startup-timing.json"
import { defaultGatewayTiming } from "./gateway-source"

describe("the host's hold of the desktop window's read", () => {
  // A connect reads the endpoint, then the credential; the host may hold each
  // for `desktopStartupWaitMs` (`desktop_startup_wait` in src-tauri). Both
  // holds fit in the connect's budget, so a startup that runs past them is the
  // host's refusal, not the window giving up on a read still held.
  it("fits twice in one connect's call budget", () => {
    expect(timing.desktopStartupWaitMs).toBeGreaterThan(0)
    expect(2 * timing.desktopStartupWaitMs).toBeLessThan(defaultGatewayTiming.callMs)
  })
})
