// One page and one ordered command stream are shared by the viewer and agent.
export class ExperimentFailure extends Error {
  constructor(code, cause) {
    super(code, { cause })
    this.code = code
  }
}

export function createSession(page, home) {
  let state = "ready"
  let pending = 0
  let tail = Promise.resolve()
  const targets = Object.freeze({
    home,
    next: `${home}next`,
    unavailable: "http://127.0.0.1:1/",
  })
  const operations = {
    navigate: async ({ target }) => {
      if (typeof target !== "string" || !Object.hasOwn(targets, target))
        throw new ExperimentFailure("invalid_target")
      await page.goto(targets[target], { waitUntil: "domcontentloaded", timeout: 5000 })
    },
    click: async ({ x, y }) => {
      if (![x, y].every(Number.isFinite) || x < 0 || y < 0 || x >= 1000 || y >= 700)
        throw new ExperimentFailure("invalid_coordinates")
      await page.mouse.click(x, y)
    },
    text: async ({ text }) => {
      if (typeof text !== "string" || text.length > 256)
        throw new ExperimentFailure("invalid_text")
      await page.keyboard.insertText(text)
    },
    key: async ({ key }) => {
      if (
        ![
          "Enter",
          "Tab",
          "Backspace",
          "Escape",
          "ArrowLeft",
          "ArrowRight",
          "ArrowUp",
          "ArrowDown",
        ].includes(key)
      )
        throw new ExperimentFailure("invalid_key")
      await page.keyboard.press(key)
    },
    wheel: async ({ deltaY }) => {
      if (!Number.isFinite(deltaY) || Math.abs(deltaY) > 2000)
        throw new ExperimentFailure("invalid_scroll")
      await page.mouse.wheel(0, deltaY)
    },
    increment: async () => page.locator("#counter").click({ timeout: 3000 }),
    frame: async () => page.screenshot({ type: "jpeg", quality: 65, timeout: 5000 }),
  }
  function run(command) {
    if (state !== "ready") return Promise.reject(new ExperimentFailure("session_closed"))
    if (
      !command ||
      typeof command !== "object" ||
      typeof command.type !== "string" ||
      !Object.hasOwn(operations, command.type)
    )
      return Promise.reject(new ExperimentFailure("invalid_command"))
    if (pending >= 16) return Promise.reject(new ExperimentFailure("queue_full"))
    pending++
    const result = tail.then(async () => {
      // Closing fences queued commands as well as later arrivals.
      if (state !== "ready") throw new ExperimentFailure("session_closed")
      try {
        return await operations[command.type](command)
      } catch (error) {
        if (error instanceof ExperimentFailure) throw error
        throw new ExperimentFailure("browser_operation_failed", error)
      }
    })
    tail = result
      .catch(() => {})
      .finally(() => {
        pending--
      })
    return result
  }
  return {
    run,
    status: () => ({ state, pending }),
    close: async () => {
      state = "closed"
      await tail
    },
    disconnected: () => {
      state = "closed"
    },
  }
}
