function wireStartupActions(root) {
  if (!root || root.dataset.nessaStartupWired === "1") return
  root.dataset.nessaStartupWired = "1"
  root.addEventListener("click", (event) => {
    const target = event.target
    const action =
      target && target.closest ? target.closest("[data-nessa-startup-action]") : null
    if (!action || !root.contains(action)) return
    const name = action.getAttribute("data-nessa-startup-action")
    if (name === "copy") {
      const code = action.textContent ? action.textContent.trim() : ""
      const clipboard = navigator.clipboard
      if (clipboard && typeof clipboard.writeText === "function") {
        clipboard.writeText(code).catch(() => {})
      }
      return
    }
    const internals = window.__TAURI_INTERNALS__
    if (!internals || typeof internals.invoke !== "function") return
    if (name === "restart") void internals.invoke("restart_nessa")
    if (name === "quit") void internals.invoke("quit_nessa")
  })
}
