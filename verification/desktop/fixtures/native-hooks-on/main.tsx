/**
 * Settings › Privacy & Permissions › Native hooks with a gateway connected.
 * `?state=on` (the default) has Claude's choice on. `?state=unset` leaves
 * Claude out of the catalog. `?state=failed` refuses the next change, so
 * the row says it could not change. Codex stays off and configured.
 */
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import type { NativeHooksGateway } from "../../../../src/desktop/settings/adapters/native-hooks-gateway"
import { NativeHooksProvider } from "../../../../src/desktop/settings"
import { SettingsHost } from "../../../../src/desktop/settings/ui/settings-view"
import { DesktopIconFamilyProvider } from "../../../../src/desktop/ui/icons"
import { testStore } from "../../../../src/desktop/workspace/testing"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "../../../../src/desktop/styles.css"

const state = new URLSearchParams(window.location.search).get("state") ?? "on"
const claudeConfigured = state !== "unset"
const gateway: NativeHooksGateway = {
  read: () =>
    Promise.resolve({
      ok: true,
      choice: {
        claude: true,
        codex: false,
        claudeConfigured,
        codexConfigured: true,
      },
    }),
  set: (agent, enabled) =>
    state === "failed"
      ? Promise.resolve({ ok: false })
      : Promise.resolve({ ok: true, agent, enabled }),
}

const root = document.getElementById("root")
if (!root) throw new Error("missing #root")

createRoot(root).render(
  <Provider store={testStore()}>
    <DesktopIconFamilyProvider>
      <NativeHooksProvider gateway={gateway}>
        <SettingsHost hostKind="browser" browserSurface open onClose={() => {}} />
      </NativeHooksProvider>
    </DesktopIconFamilyProvider>
  </Provider>,
)
