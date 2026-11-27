import { Input } from "@nessa-ui/react/input"
import {
  createContext,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react"
import type { InstalledApp } from "@nessa/client"
import {
  confirmAppInstall,
  onInstalledAppsChanged,
  openInstalledApp,
  placeInstalledApp,
  removeInstalledApp,
  runningInstalledApps,
  stopInstalledApp,
} from "../../../host"
import { Group, ItemRow, RowAction } from "../../settings/ui/settings-controls"
import {
  refusalWords,
  type InstalledAppsGateway,
} from "../adapters/installed-apps-gateway"

/**
 * Settings › Workspace › Apps: one row per installed app, with its tile, its
 * name and Open, and one row to make a new app.
 *
 * ```text
 * New app {id} ── apps.starter ──▶ native Install/Cancel (host, names the digest)
 *     Cancel ──▶ nothing is sent, nothing kept
 *     Install ──▶ apps.install {package, reviewedDigest} ── gateway verifies again
 *         ──▶ host places its launcher (an app of its own, with its icon)
 * Open ── apps.read ──▶ host verifies again, places, runs it (own process, no network)
 * Stop ── host ends it      Uninstall ── Stop, apps.uninstall, launcher removed
 * ```
 *
 * The gateway is the one authority on what is installed; the host on which
 * apps are running. This tab only asks them and shows their answers.
 */
interface InstalledAppsContextValue {
  readonly gateway: InstalledAppsGateway | undefined
  /** A fresh request id per change, so a retried change is the same one. */
  readonly requestId: () => string
}

const InstalledAppsContext = createContext<InstalledAppsContextValue>({
  gateway: undefined,
  requestId: () => crypto.randomUUID(),
})

export function InstalledAppsProvider({
  gateway,
  requestId,
  children,
}: {
  gateway: InstalledAppsGateway | undefined
  requestId?: () => string
  children: ReactNode
}) {
  return (
    <InstalledAppsContext.Provider
      value={{ gateway, requestId: requestId ?? (() => crypto.randomUUID()) }}
    >
      {children}
    </InstalledAppsContext.Provider>
  )
}

type Listing =
  | { readonly kind: "reading" }
  | { readonly kind: "unread" }
  | { readonly kind: "ready"; readonly apps: readonly InstalledApp[] }

/** What happened last, said under the list. */
type Said = { readonly tone: "problem"; readonly words: string }

export function InstalledAppsTab() {
  const { gateway, requestId } = useContext(InstalledAppsContext)
  const [listing, setListing] = useState<Listing>({ kind: "reading" })
  const [running, setRunning] = useState<readonly string[]>([])
  const [busy, setBusy] = useState<string>()
  const [said, setSaid] = useState<Said>()
  const [retry, setRetry] = useState(0)
  const [draft, setDraft] = useState("")
  const saidId = useId()
  const revision = useRef(0)

  useEffect(() => {
    if (!gateway) return
    const current = ++revision.current
    setListing({ kind: "reading" })
    gateway.installed().then(
      (apps) => {
        if (current === revision.current) setListing({ kind: "ready", apps })
      },
      () => {
        if (current === revision.current) setListing({ kind: "unread" })
      },
    )
    return () => {
      revision.current += 1
    }
  }, [gateway, retry])

  useEffect(() => {
    let live = true
    const refresh = () =>
      void runningInstalledApps().then((ids) => {
        if (live) setRunning(ids)
      })
    refresh()
    const stop = onInstalledAppsChanged(refresh)
    return () => {
      live = false
      void stop.then((unlisten) => unlisten())
    }
  }, [])

  const reread = () => setRetry((value) => value + 1)
  const run = async (key: string, work: () => Promise<void>) => {
    if (busy) return
    setBusy(key)
    setSaid(undefined)
    try {
      await work()
    } catch (cause) {
      setSaid({ tone: "problem", words: refusalWords(cause) })
    } finally {
      setBusy(undefined)
    }
  }

  // Made from the starter and installed in one step: the system's own
  // dialog, which names the package's digest, is the review.
  const create = () =>
    run("create", async () => {
      if (!gateway) return
      const { package: pkg, review } = await gateway.starter(draft.trim(), "1.0.0")
      if (!(await confirmAppInstall(review))) return
      try {
        await gateway.install(pkg, review.digest, requestId())
      } finally {
        reread()
      }
      setDraft("")
      // Its launcher: an app of its own in Applications or the menu.
      await placeInstalledApp(review.appId, pkg, review.digest)
    })

  const open = (appId: string) =>
    run(`open:${appId}`, async () => {
      if (!gateway) return
      const { package: pkg, review } = await gateway.read(appId)
      await openInstalledApp(appId, pkg, review.digest)
    })

  const stop = (appId: string) =>
    run(`stop:${appId}`, async () => {
      await stopInstalledApp(appId)
    })

  const uninstall = (appId: string) =>
    run(`uninstall:${appId}`, async () => {
      if (!gateway) return
      // The app stops first, so none runs from bytes the gateway removed,
      // and its launcher goes once the gateway has removed it.
      await stopInstalledApp(appId)
      try {
        await gateway.uninstall(appId, requestId())
      } finally {
        reread()
      }
      await removeInstalledApp(appId)
    })

  const ready = !!gateway && listing.kind === "ready"
  const apps = listing.kind === "ready" ? listing.apps : []
  const status = !gateway
    ? "Connect a gateway"
    : listing.kind === "reading"
      ? "Loading…"
      : listing.kind === "unread"
        ? "Couldn’t read your apps."
        : undefined

  return (
    <Group>
      {apps.length === 0 && (
        <ItemRow
          label={status ?? "No apps yet"}
          unavailable={!ready}
          control={
            listing.kind === "unread" ? (
              <RowAction onClick={reread}>Try again</RowAction>
            ) : undefined
          }
        />
      )}
      {apps.map((app) => {
        const running_ = running.includes(app.appId)
        return (
          <ItemRow
            key={app.appId}
            leading={<AppTile name={app.appId} />}
            label={app.appId}
            data-installed-app={app.appId}
            data-installed-app-running={running_ ? "" : undefined}
            control={
              <span className="settings-installed-app-actions">
                {running_ ? (
                  <RowAction
                    disabled={!!busy}
                    onClick={() => void stop(app.appId)}
                    data-installed-app-stop={app.appId}
                  >
                    Stop
                  </RowAction>
                ) : (
                  <RowAction
                    disabled={!!busy}
                    onClick={() => void open(app.appId)}
                    data-installed-app-open={app.appId}
                  >
                    Open
                  </RowAction>
                )}
                <RowAction
                  icon="delete"
                  disabled={!!busy}
                  onClick={() => void uninstall(app.appId)}
                  data-installed-app-uninstall={app.appId}
                >
                  Uninstall
                </RowAction>
              </span>
            }
          />
        )
      })}
      <ItemRow
        label={
          <Input
            className="settings-terminal-rule-field settings-new-app-field"
            type="text"
            aria-label="New app"
            placeholder="New app, such as my-notes"
            data-create-app-id=""
            disabled={!ready}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key !== "Enter") return
              event.preventDefault()
              void create()
            }}
          />
        }
        setting="create-app"
        unavailable={!ready}
        control={
          <RowAction
            disabled={!!busy || draft.trim() === ""}
            onClick={() => void create()}
            data-create-app=""
          >
            Create
          </RowAction>
        }
      />
      {said && (
        <p
          id={saidId}
          role="alert"
          className="settings-boat-note"
          data-installed-apps-said={said.tone}
        >
          {said.words}
        </p>
      )}
    </Group>
  )
}

/** An app's tile: its first letter until apps carry their own icon. */
function AppTile({ name }: { name: string }) {
  return (
    <span className="settings-app-tile" aria-hidden="true">
      {name.slice(0, 1).toUpperCase()}
    </span>
  )
}
