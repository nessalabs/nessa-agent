/** A real native vault-session child over anonymous pipes, owned by this test's
 * simulated app. No OS credential-store operation or real account is involved. */
import { spawn } from "node:child_process"
import { join } from "node:path"
import { repoRoot } from "../../../../scripts/mcp-test-server/local-gateway.mjs"
export const VAULT_PASSPHRASE = "disposable Nessa vault verification passphrase"
export function appVault({ endpoint, credential }) {
  let child = null,
    buffer = "",
    waiting = null,
    ready = null,
    closed = false,
    busy = false
  function reject() {
    waiting?.reject(new Error("Vault child connection ended"))
    waiting = null
    ready?.reject(new Error("Vault child did not become ready"))
    ready = null
  }
  async function start() {
    if (closed) throw new Error("Vault app is closed")
    if (child) return
    const address = new URL(typeof endpoint === "function" ? endpoint() : endpoint).host
    const input = typeof credential === "function" ? credential() : credential
    const pending = deferred()
    ready = pending
    child = spawn(
      process.env.MCP_LIVE_NESSA ?? join(repoRoot, "target/debug/nessa"),
      ["vault", "session"],
      { stdio: ["pipe", "pipe", "pipe"] },
    )
    const owned = child
    owned.stdout.setEncoding("utf8")
    owned.stdout.on("data", (chunk) => {
      if (child !== owned) return
      buffer += chunk
      if (buffer.length > 32768) {
        owned.kill("SIGKILL")
        reject()
        return
      }
      for (let end; (end = buffer.indexOf("\n")) >= 0;) {
        const line = buffer.slice(0, end)
        buffer = buffer.slice(end + 1)
        let reply
        try {
          reply = JSON.parse(line)
        } catch {
          reject()
          owned.kill("SIGKILL")
          return
        }
        if (reply.ready === true) {
          ready?.resolve()
          ready = null
        } else {
          waiting?.resolve(reply)
          waiting = null
        }
      }
    })
    owned.on("exit", () => {
      if (child === owned) {
        child = null
        buffer = ""
        reject()
      }
    })
    owned.on("error", () => {
      if (child === owned) reject()
    })
    owned.stdin.on("error", () => {
      if (child === owned) reject()
    })
    owned.stderr.resume()
    owned.stdin.write(JSON.stringify({ address, credential: input }) + "\n")
    await bounded(pending.promise, 15000, () => owned.kill("SIGKILL"))
  }
  async function exchange(request) {
    await start()
    if (waiting) throw new Error("Vault verification request overlap")
    const pending = deferred()
    waiting = pending
    const owned = child
    owned.stdin.write(JSON.stringify(request) + "\n")
    return bounded(pending.promise, 15000, () => {
      owned.kill("SIGKILL")
      reject()
    })
  }
  async function command(request) {
    if (busy) return { ok: false, code: "vault_busy" }
    busy = true
    try {
      let reply
      try {
        reply = await exchange(request)
      } catch (error) {
        await stop()
        if (request.operation === "status") return await exchange(request)
        throw error
      }
      if (reply.ok === false && reply.code === "vault_unavailable") {
        await stop()
        if (request.operation === "status") reply = await exchange(request)
      }
      return reply
    } finally {
      busy = false
    }
  }
  async function stop() {
    const owned = child
    if (!owned) return
    const exited = new Promise((resolve) => owned.once("exit", resolve))
    owned.stdin.end()
    await bounded(exited, 5000, () => owned.kill("SIGKILL"))
  }
  return {
    command,
    stop,
    close: async () => {
      closed = true
      await stop()
    },
    async unlock() {
      let reply = await command({ operation: "status" })
      if (!reply.ok) throw new Error("Vault fixture status failed")
      if (reply.status.state !== "unlocked") {
        reply = await command({
          operation: reply.status.state === "unconfigured" ? "setup" : "unlock",
          passphrase: VAULT_PASSPHRASE,
        })
        if (!reply.ok) throw new Error("Vault fixture unlock failed")
      }
    },
    async install(page) {
      await page.exposeFunction("__NESSA_VAULT_COMMAND", command)
    },
  }
}
function deferred() {
  let resolve, reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
async function bounded(promise, ms, expire) {
  let timer
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          expire()
          reject(new Error("Vault verification timed out"))
        }, ms)
      }),
    ])
  } finally {
    clearTimeout(timer)
  }
}
/** Protected-use flows enter the public disposable passphrase only when prompted. */
export async function unlockVault(page, { screenshot, expectedContainer } = {}) {
  const dialog = page.locator('[data-vault-dialog="unlock"]')
  if (await dialog.isVisible()) {
    const panel = page.getByRole("dialog", { name: "Unlock Nessa", exact: true })
    if (
      expectedContainer &&
      !(await panel.evaluate(
        (node, selector) => Boolean(node.closest(selector)),
        expectedContainer,
      ))
    )
      throw new Error("Vault dialog left its requesting portal container")
    const onTop = await panel.evaluate((node) => {
      const box = node.getBoundingClientRect()
      return node.contains(
        document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2),
      )
    })
    if (!onTop) throw new Error("Vault dialog is hidden below its requesting surface")
    if (screenshot) await panel.screenshot({ path: screenshot })
    await dialog.getByLabel("Passphrase", { exact: true }).fill(VAULT_PASSPHRASE)
    await dialog.getByRole("button", { name: "Unlock", exact: true }).click()
    await dialog.waitFor({ state: "hidden" })
  }
}
