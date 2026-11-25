import { useEffect, useRef, useState } from "react"
import { LoadingText } from "@nessa-ui/react/loading-text"
import { useWorkspaceSelector } from "../workspace/adapters/store/hooks"
import { selectFailure } from "../workspace/adapters/store/selectors"
import { startupFailureCode } from "../workspace/ui/startup-failure"
import "./connecting-splash.css"

/** How long the splash takes to fade once the gateway answers. */
const LEAVE_MS = 180

/**
 * Whether the window is still waiting for its gateway's first answer: the
 * workspace has not read its index yet (or the host said the gateway is still
 * starting), and no startup failure has been shown
 * since the window opened. After a failure the startup screen is the one that
 * covers the window, so a retry does not swap the two back and forth.
 */
export function useConnecting(): boolean {
  const status = useWorkspaceSelector((state) => state.workspace.status)
  const failure = useWorkspaceSelector(selectFailure)
  const failedOnce = useRef(false)
  if (startupFailureCode(failure) !== null) failedOnce.current = true
  // A host that is still starting the gateway past its hold refuses the read
  // as `still-starting`: no failure, the window keeps asking, and connects.
  const waiting =
    status === "loading" || (status === "failed" && failure === "still-starting")
  return waiting && !failedOnce.current
}

/**
 * The quiet cover over the window while it connects to the gateway, so the
 * empty workspace ("No sessions yet") and anything that asks the gateway,
 * such as unlocking, never show before there is a gateway to answer. It fades
 * out when the workspace is read; it draws only the word, shimmering.
 */
export function ConnectingSplash({ connecting }: { connecting: boolean }) {
  const [leaving, setLeaving] = useState(false)
  const [shown, setShown] = useState(connecting)
  useEffect(() => {
    if (connecting) {
      setShown(true)
      setLeaving(false)
      return
    }
    if (!shown) return
    setLeaving(true)
    const timer = window.setTimeout(() => {
      setShown(false)
      setLeaving(false)
    }, LEAVE_MS)
    return () => window.clearTimeout(timer)
  }, [connecting, shown])
  if (!shown) return null
  return (
    <main
      className="connecting-splash"
      data-nessa-connecting=""
      data-leaving={leaving || undefined}
      data-tauri-drag-region
      aria-busy="true"
    >
      <LoadingText className="connecting-splash-text">Connecting</LoadingText>
    </main>
  )
}
