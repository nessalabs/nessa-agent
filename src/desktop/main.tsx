import * as React from "react"
import { createRoot } from "react-dom/client"
import { host } from "../host"
import { DesktopApp } from "./ui/desktop-app"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

createRoot(container).render(
  <React.StrictMode>
    <DesktopApp hostKind={host.kind} browserSurface={host.kind === "browser"} />
  </React.StrictMode>,
)
