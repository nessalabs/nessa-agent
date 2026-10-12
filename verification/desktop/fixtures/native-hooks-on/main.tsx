/**
 * Settings › Privacy & Permissions › Access with a gateway connected and
 * Claude's hooks choice on. `settings-page.mjs` opens Privacy & Permissions
 * and screenshots the trust warning. Codex stays off. Both agents are in
 * the catalog, so neither row says it isn't set up.
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

const gateway: NativeHooksGateway = {
  read: () =>
    Promise.resolve({
      ok: true,
      choice: {
        claude: true,
        codex: false,
        claudeConfigured: true,
        codexConfigured: true,
      },
    }),
  set: (agent, enabled) => Promise.resolve({ ok: true, agent, enabled }),
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
