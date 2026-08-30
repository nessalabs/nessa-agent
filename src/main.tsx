import * as React from "react"
import { createRoot } from "react-dom/client"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

import { App } from "./app"

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

createRoot(container).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
)
