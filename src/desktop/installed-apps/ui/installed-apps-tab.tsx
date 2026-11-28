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
import type { AppReview, AppServerRuntime, InstalledApp } from "@nessa/client"
import {
  confirmAppInstall,
  handInstalledAppCredential,
  installedAppHasCredential,
  keepOnlyInstalledApps,
  onInstalledAppsChanged,
  openInstalledApp,
  placeInstalledApp,
  removeInstalledApp,
  runningInstalledApps,
  stopInstalledApp,
} from "../../../host"
import { Choice, Group, ItemRow, RowAction } from "../../settings/ui/settings-controls"
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
 *         ──▶ a dynamic app (it runs node or bun): apps.issueCredential ──▶ host keeps
 *             the credential in its launcher, for its own server to sign in with
 * Open ── apps.read ──▶ host verifies again, places, runs it (own process; a
 *         dynamic app's server is the one thing its page reaches) ── a dynamic
 *         app whose launcher holds no credential is issued one first
 * Stop ── host ends it      Uninstall ── Stop, apps.uninstall, launcher removed
 * each list read ── host keeps only the listed apps' launchers
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

/** What a new app is: a page, or a page with a server that runtime runs. */
type Kind = "page" | AppServerRuntime

const KINDS: readonly { id: Kind; label: string }[] = [
  { id: "page", label: "Page" },
  { id: "node", label: "Node server" },
  { id: "bun", label: "Bun server" },
]

/** Whether `review`'s app runs a server that signs in to Nessa. */
function signsIn(review: AppReview): boolean {
  return review.server !== undefined && review.requestedCapabilities.length > 0
}

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
  const [kind, setKind] = useState<Kind>("page")
  const saidId = useId()
  const revision = useRef(0)
  // Changes started here, so a list read before one started is never used
  // to remove launchers. A read a change starts comes after its gateway step.
  const changes = useRef(0)

  useEffect(() => {
    if (!gateway) return
    const current = ++revision.current
    const since = changes.current
    setListing({ kind: "reading" })
    gateway.installed().then(
      (apps) => {
        if (current !== revision.current) return
        setListing({ kind: "ready", apps })
        // A launcher the gateway no longer lists goes: an uninstall that
        // failed halfway, or one made from another client.
        if (since === changes.current)
          void keepOnlyInstalledApps(apps.map((app) => app.appId)).catch(() => undefined)
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
    changes.current += 1
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
      const { package: pkg, review } = await gateway.starter(
        draft.trim(),
        "1.0.0",
        kind === "page" ? undefined : kind,
      )
      if (!(await confirmAppInstall(review, pkg))) return
      try {
        await gateway.install(pkg, review.digest, requestId())
      } finally {
        reread()
      }
      // Installed again over an app that is open, it stops: issuing ends
      // the credential its server holds.
      await stopInstalledApp(review.appId)
      setDraft("")
      // Its launcher: an app of its own in Applications or the menu.
      await placeInstalledApp(review.appId, pkg, review.digest)
      if (signsIn(review)) await handCredential(review.appId)
    })

  // The credential goes from the gateway to the app's launcher and nowhere
  // else; issuing again ends the one before.
  const handCredential = async (appId: string) => {
    if (!gateway) return
    const issued = await gateway.issueCredential(appId, requestId())
    await handInstalledAppCredential(appId, issued.credential, issued.expiresAt)
  }

  const open = (appId: string) =>
    run(`open:${appId}`, async () => {
      if (!gateway) return
      const { package: pkg, review } = await gateway.read(appId)
      await placeInstalledApp(appId, pkg, review.digest)
      // One missing, or ending within a month, is issued again, but never
      // under a server that is running: issuing ends the one it signed in
      // with. The host is asked now, not the list drawn, since an app opened
      // from the Dock shows here only after a moment. It is renewed on the
      // Open after it stops.
      if (
        signsIn(review) &&
        !(await installedAppHasCredential(appId)) &&
        !(await runningInstalledApps()).includes(appId)
      )
        await handCredential(appId)
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
                    {busy === `stop:${app.appId}` ? "Stopping…" : "Stop"}
                  </RowAction>
                ) : (
                  <RowAction
                    disabled={!!busy}
                    onClick={() => void open(app.appId)}
                    data-installed-app-open={app.appId}
                  >
                    {busy === `open:${app.appId}` ? "Opening…" : "Open"}
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
          <span className="settings-installed-app-actions">
            <Choice
              key="kind"
              label="Kind of app"
              value={kind}
              options={KINDS}
              onChange={setKind}
              data-create-app-kind=""
            />
            <RowAction
              disabled={!!busy || draft.trim() === ""}
              onClick={() => void create()}
              data-create-app=""
            >
              {busy === "create" ? "Creating…" : "Create"}
            </RowAction>
          </span>
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
