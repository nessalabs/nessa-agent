import * as React from "react"
import { createRoot } from "react-dom/client"
import { host } from "../host"
import { DesktopIconFamilyProvider } from "./ui/icons"
import { VariantSwitcher } from "./ui/spike/variant-switcher"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

createRoot(container).render(
  <React.StrictMode>
    {/* Every icon in the window resolves through the family chosen in Settings. */}
    <DesktopIconFamilyProvider>
      <VariantSwitcher hostKind={host.kind} browserSurface={host.kind === "browser"} />
    </DesktopIconFamilyProvider>
  </React.StrictMode>,
)
