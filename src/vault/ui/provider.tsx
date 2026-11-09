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
  source: undefined as VaultSource | undefined,
  ensure: async (_owner: symbol, _container: HTMLElement | null) => true,
  release: (_owner: symbol) => {},
  beginChange: (_owner: symbol): boolean => false,
  endChange: (_owner: symbol) => {},
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
  const foregroundChange = useRef<symbol | null>(null)
  const requestUnlock = useCallback(
    (showLoading = false) => {
      if (foregroundChange.current !== null) return Promise.resolve(false)
      return controller.ensure(showLoading)
    },
    [controller],
  )
  const ensure = useCallback(
    (owner: symbol, target: HTMLElement | null) => {
      const request = requestUnlock()
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
    [requestUnlock],
  )
  const beginChange = useCallback(
    (owner: symbol) => {
      if (foregroundChange.current !== null) return foregroundChange.current === owner
      const current = controller.snapshot()
      if ((current.kind === "setup" || current.kind === "unlock") && current.busy)
        return false
      foregroundChange.current = owner
      controller.cancel()
      placement.current = null
      setContainer(null)
      return true
    },
    [controller],
  )
  const endChange = useCallback((owner: symbol) => {
    if (foregroundChange.current === owner) foregroundChange.current = null
  }, [])
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
  const context = useMemo(
    () => ({ ensure, release, source, beginChange, endChange }),
    [ensure, release, source, beginChange, endChange],
  )
  useEffect(() => {
    controller.activate()
    if (setup || autoUnlock) void requestUnlock(setup)
    return () => controller.dispose()
  }, [controller, setup, autoUnlock, requestUnlock])
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
/** The explicit settings control shares this provider's owning connection. */
export function useVaultSource() {
  return useContext(VaultContext).source
}
/** One explicit change dialog owns foreground presentation through recovery. */
export function useVaultChangeOwnership() {
  const { beginChange, endChange } = useContext(VaultContext)
  const owner = useRef(Symbol("vault-change-requester")).current
  useEffect(() => () => endChange(owner), [endChange, owner])
  return useMemo(
    () => ({
      begin: () => beginChange(owner),
      end: () => endChange(owner),
    }),
    [beginChange, endChange, owner],
  )
}
