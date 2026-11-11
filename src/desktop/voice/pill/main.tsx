/**
 * The call pill's page (`call.html`): the small window that floats over
 * other apps while a voice call is open (`src-tauri/src/call_pill.rs`). It
 * holds no call. It draws the picture the desktop window sends and hands
 * presses back; with no picture it draws nothing, and its window is hidden.
 */
import React, { useEffect, useState } from "react"
import { createRoot } from "react-dom/client"
import {
  dragCallPill,
  onCallPillPicture,
  pressCallPill,
  type CallPicture,
} from "../../../host"
import "../../styles.css"
import { CallPill } from "./call-pill"

function CallPillPage() {
  const [picture, setPicture] = useState<CallPicture | null>(null)
  useEffect(() => {
    // Listening is asked for, and answers later: a page gone before then
    // stops the listener as soon as it is there.
    let stop: (() => void) | null = null
    let stopped = false
    void onCallPillPicture(setPicture).then(
      (unlisten) => (stopped ? unlisten() : (stop = unlisten)),
      (error: unknown) => console.warn("The call pill cannot hear its call", error),
    )
    return () => {
      stopped = true
      stop?.()
    }
  }, [])
  return (
    <div className="call-pill-page" data-surface="call">
      {picture ? (
        <CallPill
          picture={picture}
          onPress={(press) =>
            void pressCallPill(press).catch((error: unknown) =>
              console.warn("The call pill cannot reach its call", error),
            )
          }
          onDrag={() => void dragCallPill().catch(() => {})}
        />
      ) : null}
    </div>
  )
}

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

createRoot(container).render(
  <React.StrictMode>
    <CallPillPage />
  </React.StrictMode>,
)
