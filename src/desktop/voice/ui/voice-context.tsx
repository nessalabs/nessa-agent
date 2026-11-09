/**
 * The window's voice call, for whichever pane shows it: the controller
 * composition made (`main.tsx`), and its snapshot as React state. Without a
 * provider — the sample preview, a test — there is no call to start, and
 * the composer offers no voice button.
 */
import { createContext, useContext, useSyncExternalStore, type ReactNode } from "react"
import type { VoiceController, VoiceSnapshot } from "../application/voice-controller"

const VoiceContext = createContext<VoiceController | undefined>(undefined)

export function VoiceProvider({
  controller,
  children,
}: {
  controller: VoiceController | undefined
  children: ReactNode
}) {
  return <VoiceContext.Provider value={controller}>{children}</VoiceContext.Provider>
}

export function useVoiceController(): VoiceController | undefined {
  return useContext(VoiceContext)
}

const noSubscription = () => () => {}
const noCall = () => undefined

/** The call as it stands; undefined without a controller. */
export function useVoiceSnapshot(): VoiceSnapshot | undefined {
  const controller = useVoiceController()
  return useSyncExternalStore(
    controller ? (listener) => controller.subscribe(listener) : noSubscription,
    controller ? () => controller.snapshot() : noCall,
  )
}
