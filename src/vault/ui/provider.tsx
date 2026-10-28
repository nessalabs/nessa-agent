import {
  createContext,
  useContext,
  useCallback,
  useRef,
  useState,
  useEffect,
  useMemo,
  useSyncExternalStore,
  type ReactNode,
} from "react"
import { VaultController } from "../application/controller"
import type { VaultSource } from "../application/ports"
import { VaultDialog } from "./dialog"
import { usePortalContainer } from "@nessa-ui/react/lib/portal-container"
const VaultContext = createContext({
  ensure: async (_owner: symbol, _container: HTMLElement | null) => true,
  release: (_owner: symbol) => {},
})
/** UI gate only: a missing provider adds no UI behavior; the gateway still owns
 * cryptographic admission. Native composition supplies its app-owned source. */
export function VaultProvider({
  source,
  setup = false,
  autoUnlock = false,
  children,
}: {
  source: VaultSource | undefined
  setup?: boolean
  autoUnlock?: boolean
  children: ReactNode
}) {
  return source ? (
    <MountedVault
      key={setup ? "setup" : "workspace"}
      source={source}
      setup={setup}
      autoUnlock={autoUnlock}
    >
      {children}
    </MountedVault>
  ) : (
    children
  )
}
function MountedVault({
  source,
  setup,
  autoUnlock,
  children,
}: {
  source: VaultSource
  setup: boolean
  autoUnlock: boolean
  children: ReactNode
}) {
  const controller = useMemo(() => new VaultController(source), [source])
  const view = useSyncExternalStore(controller.subscribe, controller.snapshot)
  const [container, setContainer] = useState<HTMLElement | null>(null)
  const placement = useRef<{ owner: symbol; request: Promise<boolean> } | null>(null)
  const ensure = useCallback(
    (owner: symbol, target: HTMLElement | null) => {
      const request = controller.ensure()
      if (placement.current?.request !== request || placement.current.owner === owner) {
        placement.current = { owner, request }
        setContainer(target)
      }
      void request.then(() => {
        if (placement.current?.request === request) {
          placement.current = null
          setContainer(null)
        }
      })
      return request
    },
    [controller],
  )
  const release = useCallback(
    (owner: symbol) => {
      if (placement.current?.owner === owner) {
        placement.current = null
        setContainer(null)
        controller.cancel()
      }
    },
    [controller],
  )
  const context = useMemo(() => ({ ensure, release }), [ensure, release])
  useEffect(() => {
    controller.activate()
    if (setup || autoUnlock) void controller.ensure(setup)
    return () => controller.dispose()
  }, [controller, setup, autoUnlock])
  useEffect(() => {
    window.addEventListener("focus", controller.refreshPending)
    return () => window.removeEventListener("focus", controller.refreshPending)
  }, [controller])
  return (
    <VaultContext.Provider value={context}>
      {children}
      {view.kind !== "closed" ? (
        <VaultDialog
          view={view}
          container={container}
          onSubmit={controller.submit}
          onCancel={controller.cancel}
          onRetry={controller.retry}
        />
      ) : null}
    </VaultContext.Provider>
  )
}
export function useVaultUnlock() {
  const context = useContext(VaultContext)
  const container = usePortalContainer()
  const owner = useRef(Symbol("vault-requester")).current
  useEffect(() => () => context.release(owner), [context, owner])
  return useCallback(() => context.ensure(owner, container), [context, owner, container])
}
