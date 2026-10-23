/** Production settings rows over controlled ports; no gateway or OS prompt. */
import * as React from "react"
import { createRoot } from "react-dom/client"
import {
  NativeHooksProvider,
  NativeHooksRows,
} from "../../../../src/desktop/settings/ui/native-hooks-rows"
import type {
  NativeHooksGateway,
  NativeHooksRead,
  NativeHooksWrite,
} from "../../../../src/desktop/settings/adapters/native-hooks-gateway"
import { ReviewNoticeRows } from "../../../../src/desktop/settings/ui/review-notice-rows"
import {
  bindNoticeAccess,
  bindWaitingReviewNotices,
  browserNoticeAccess,
  type NoticeAccess,
} from "../../../../src/desktop/workspace/adapters/gateway/review-notices"
import "../../../../src/styles.css"

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
const saveA = deferred<NativeHooksWrite>()
const readA = deferred<NativeHooksRead>()
const saveB = deferred<NativeHooksWrite>()
const counts = {
  readsA: 0,
  readsB: 0,
  writesA: 0,
  writesB: 0,
  asksA: 0,
  asksB: 0,
  replayA: 0,
  replayB: 0,
}
function NativeHooks() {
  const [, paint] = React.useReducer((n: number) => n + 1, 0)
  const [second, setSecond] = React.useState(false)
  const a = React.useMemo<NativeHooksGateway>(
    () => ({
      read: () => {
        counts.readsA++
        paint()
        return counts.readsA === 1
          ? Promise.resolve({ ok: true, choice: { claude: false, codex: false } })
          : readA.promise
      },
      set: () => {
        counts.writesA++
        paint()
        return saveA.promise
      },
    }),
    [],
  )
  const b = React.useMemo<NativeHooksGateway>(
    () => ({
      read: async () => {
        counts.readsB++
        paint()
        return { ok: true, choice: { claude: false, codex: false } }
      },
      set: () => {
        counts.writesB++
        paint()
        return saveB.promise
      },
    }),
    [],
  )
  return (
    <section aria-label="Native hook ownership">
      <NativeHooksProvider gateway={second ? b : a}>
        <NativeHooksRows />
      </NativeHooksProvider>
      <output aria-label="Native request counts">{JSON.stringify(counts)}</output>
      <button onClick={() => saveA.resolve({ ok: true, agent: "claude", enabled: true })}>
        A acknowledges save
      </button>
      <button
        onClick={() =>
          readA.resolve({ ok: true, choice: { claude: true, codex: false } })
        }
      >
        A finishes reread
      </button>
      <button onClick={() => setSecond(true)}>Replace native source</button>
      <button onClick={() => saveB.resolve({ ok: false })}>B refuses save</button>
    </section>
  )
}
const promptA = deferred<NotificationPermission>()
const promptB = deferred<NoticeAccess>()
let repaintNotice = () => {}
class ControlledNotification {
  static permission: NotificationPermission = "default"
  static requestPermission() {
    counts.asksA++
    repaintNotice()
    return promptA.promise
  }
}
Object.defineProperty(globalThis, "Notification", {
  configurable: true,
  value: ControlledNotification,
})
bindNoticeAccess(browserNoticeAccess)
bindWaitingReviewNotices(() => {
  counts.replayA++
  repaintNotice()
})
function Notices() {
  const [, paint] = React.useReducer((n: number) => n + 1, 0)
  React.useEffect(() => {
    repaintNotice = paint
    return () => {
      repaintNotice = () => {}
    }
  }, [])
  return (
    <section aria-label="Notification permission ownership">
      <ReviewNoticeRows />
      <output aria-label="Notice request counts">{JSON.stringify(counts)}</output>
      <button onClick={() => promptA.resolve("granted")}>
        A grants notification access
      </button>
      <button
        onClick={() => {
          bindNoticeAccess({
            read: async () => "not-asked",
            ask: () => {
              counts.asksB++
              paint()
              return promptB.promise
            },
            blocked: "B blocked",
          })
          bindWaitingReviewNotices(() => {
            counts.replayB++
            paint()
          })
          paint()
        }}
      >
        Replace notification source
      </button>
      <button onClick={() => promptB.resolve("allowed")}>
        B grants notification access
      </button>
    </section>
  )
}
const root = document.getElementById("root")
if (!root) throw new Error("Missing settings fixture root")
createRoot(root).render(
  <main data-settings-action-owners>
    <h1>Settings action ownership</h1>
    <p>
      Controlled ports measure API dispatch and component lifetime. They do not show OS
      prompts or run native harnesses.
    </p>
    <NativeHooks />
    <Notices />
  </main>,
)
