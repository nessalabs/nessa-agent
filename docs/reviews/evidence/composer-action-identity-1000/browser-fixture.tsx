import { StrictMode, useState } from "react"
import { createRoot } from "react-dom/client"
import { flushSync } from "react-dom"
import { Composer } from "../../../../src/desktop/ui/composer"
import { DesktopIconFamilyProvider } from "../../../../src/desktop/ui/icons"
import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "../../../../src/desktop/styles.css"

type Mode = "send" | "stop" | "stop-disabled"
const syntheticDraft = "SYNTHETIC unsent draft"
let changeMode: (mode: Mode) => void
let changeDraft: (text: string) => void
let sent: string[] = []
let stops = 0
function Fixture() {
  const [mode, setMode] = useState<Mode>("send")
  const [draft, setDraft] = useState(syntheticDraft)
  changeMode = setMode
  changeDraft = setDraft
  return (
    <Composer
      page={false}
      onPageChange={() => {}}
      model={undefined}
      text={draft}
      onTextChange={setDraft}
      onSend={(text) => {
        sent.push(text)
        setDraft("")
      }}
      onStop={
        mode === "send"
          ? undefined
          : {
              stopping: mode === "stop-disabled",
              stop: () => {
                stops++
              },
            }
      }
    />
  )
}
const probe = {
  ready: () => typeof changeMode === "function",
  reset(mode: Mode) {
    sent = []
    stops = 0
    flushSync(() => {
      changeMode(mode)
      changeDraft(syntheticDraft)
    })
  },
  mode(mode: Mode) {
    flushSync(() => changeMode(mode))
  },
  result() {
    return {
      sent: [...sent],
      stops,
      draft: document.querySelector("textarea")?.value,
      focus: document.activeElement?.tagName,
    }
  },
}
Object.assign(window, { composerRegression: probe })
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <DesktopIconFamilyProvider>
      <Fixture />
    </DesktopIconFamilyProvider>
  </StrictMode>,
)
