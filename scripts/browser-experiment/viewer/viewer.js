let token = location.hash.slice(1)
history.replaceState(null, "", location.pathname)
const screen = document.querySelector("#screen")
const message = document.querySelector("#message")
let objectUrl
let stopped = false
const headers = () => ({ Authorization: `Bearer ${token}` })
async function request(path, options = {}) {
  const response = await fetch(path, {
    ...options,
    headers: { ...headers(), ...options.headers },
  })
  if (!response.ok) throw new Error((await response.json()).error)
  return response
}
async function command(value) {
  try {
    await request("/command", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(value),
    })
    message.textContent = "Applied to the shared browser."
  } catch (error) {
    message.textContent = `Command failed: ${error.message}`
  }
}
document
  .querySelectorAll("[data-target]")
  .forEach(
    (button) =>
      (button.onclick = () =>
        command({ type: "navigate", target: button.dataset.target })),
  )
document.querySelector("#agent").onclick = () => command({ type: "increment" })
screen.onclick = (event) => {
  const rect = screen.getBoundingClientRect()
  screen.focus()
  void command({
    type: "click",
    x: ((event.clientX - rect.left) * 1000) / rect.width,
    y: ((event.clientY - rect.top) * 700) / rect.height,
  })
}
screen.onkeydown = (event) => {
  if (
    [
      "Enter",
      "Tab",
      "Backspace",
      "Escape",
      "ArrowLeft",
      "ArrowRight",
      "ArrowUp",
      "ArrowDown",
    ].includes(event.key)
  ) {
    event.preventDefault()
    void command({ type: "key", key: event.key })
  }
}
screen.onwheel = (event) => {
  event.preventDefault()
  void command({ type: "wheel", deltaY: Math.max(-2000, Math.min(2000, event.deltaY)) })
}
document.querySelector("#text-form").onsubmit = (event) => {
  event.preventDefault()
  void command({ type: "text", text: document.querySelector("#text").value })
}
async function refresh() {
  if (stopped) return
  const start = performance.now()
  try {
    const frame = await (await request("/frame")).blob()
    const previous = objectUrl
    objectUrl = URL.createObjectURL(frame)
    screen.src = objectUrl
    if (previous) URL.revokeObjectURL(previous)
    document.querySelector("#status").textContent = "Connected"
    document.querySelector("#metrics").textContent =
      `${Math.round(performance.now() - start)} ms frame request · ${frame.size} bytes · up to 4 frames/s`
  } catch (error) {
    document.querySelector("#status").textContent = "Disconnected"
    message.textContent = `Frame unavailable: ${error.message}. Reopen the local capability URL after a viewer reload.`
    stopped = true
  }
  if (!stopped) setTimeout(refresh, 250)
}
window.addEventListener("pagehide", () => {
  stopped = true
  token = ""
  if (objectUrl) URL.revokeObjectURL(objectUrl)
})
void refresh()
