import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

import { App } from "./panel"
import { store } from "./store"

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

createRoot(container).render(
  <React.StrictMode>
    <Provider store={store}>
      <App />
    </Provider>
  </React.StrictMode>,
)
