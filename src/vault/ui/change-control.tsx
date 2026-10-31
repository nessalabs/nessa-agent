import { useEffect, useMemo, useRef, useSyncExternalStore } from "react"
import { Button } from "@nessa-ui/react/button"
import { usePortalContainer } from "@nessa-ui/react/lib/portal-container"
import { VaultChangeController } from "../application/change-controller"
import type { VaultSource } from "../application/ports"
import { useVaultChangeOwnership, useVaultSource, useVaultUnlock } from "./provider"
import { VaultChangeDialog } from "./change-dialog"
/** Explicit settings action, separate from the protected-use unlock controller. */
export function VaultPassphraseControl() {
  const source = useVaultSource()
  return source ? <MountedChangeControl source={source} /> : null
}
function MountedChangeControl({ source }: { source: VaultSource }) {
  const ensure = useVaultUnlock()
  const ownership = useVaultChangeOwnership()
  const container = usePortalContainer()
  const controller = useMemo(() => new VaultChangeController(source), [source])
  const view = useSyncExternalStore(controller.subscribe, controller.snapshot)
  const lifetime = useRef<symbol | null>(null)
  const opening = useRef<symbol | null>(null)
  useEffect(() => {
    lifetime.current = Symbol("mounted-vault-change")
    return () => {
      lifetime.current = null
      opening.current = null
      ownership.end()
      controller.dispose()
    }
  }, [controller, ownership])
  const open = async () => {
    if (opening.current || lifetime.current === null) return
    const mounted = lifetime.current
    const request = Symbol("opening-vault-change")
    opening.current = request
    try {
      if ((await ensure()) && lifetime.current === mounted && ownership.begin())
        controller.open()
    } finally {
      if (opening.current === request) opening.current = null
    }
  }
  const close = () => {
    controller.close()
    if (controller.snapshot().kind === "closed") ownership.end()
  }
  return (
    <>
      <Button
        variant="ghost"
        onClick={() => {
          void open()
        }}
      >
        Change passphrase
      </Button>
      {view.kind !== "closed" ? (
        <VaultChangeDialog
          view={view}
          container={container}
          onSubmit={controller.submit}
          onClose={close}
          onUnlock={() => {
            close()
            void ensure()
          }}
        />
      ) : null}
    </>
  )
}
