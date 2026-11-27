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
import type { CheckedAppPackage, InstalledApp } from "@nessa/client"
import {
  confirmAppInstall,
  onInstalledAppsChanged,
  openInstalledApp,
  runningInstalledApps,
  stopInstalledApp,
} from "../../../host"
import type { SandboxOrigin } from "../../widgets/app/application/ports"
import { setting } from "../../settings/model/settings-catalogue"
import { Group, ItemRow, RowAction } from "../../settings/ui/settings-controls"
import {
  refusalWords,
  type InstalledAppsGateway,
} from "../adapters/installed-apps-gateway"
import { mountStaticApp } from "../adapters/static-frame"
import { packageDocument } from "../model/app-document"

/**
 * Settings › Apps: the apps installed on this computer, and making a new one.
 *
 * ```text
 * Create {id, version} ── apps.starter ──▶ gateway verifies ──▶ Review card + preview
 * Install ── native Install/Cancel (host) ──▶ Cancel: nothing is sent, nothing kept
 *                                         └─▶ Install: apps.install {package, reviewedDigest}
 *                                                      gateway verifies again, refuses a changed package
 * Open ── apps.read ──▶ host verifies the bytes again ──▶ the app's own window (no network)
 * Stop ── host closes the window        Uninstall ── Stop, then apps.uninstall
 * ```
 *
 * The gateway is the one authority on what is installed; the host on which
 * app windows are open. This tab only asks them and shows their answers.
 */
interface InstalledAppsContextValue {
  readonly gateway: InstalledAppsGateway | undefined
  readonly sandbox: SandboxOrigin | undefined
  /** A fresh request id per change, so a retried change is the same one. */
  readonly requestId: () => string
}

const InstalledAppsContext = createContext<InstalledAppsContextValue>({
  gateway: undefined,
  sandbox: undefined,
  requestId: () => crypto.randomUUID(),
})

export function InstalledAppsProvider({
  gateway,
  sandbox,
  requestId,
  children,
}: {
  gateway: InstalledAppsGateway | undefined
  sandbox: SandboxOrigin | undefined
  requestId?: () => string
  children: ReactNode
}) {
  return (
    <InstalledAppsContext.Provider
      value={{ gateway, sandbox, requestId: requestId ?? (() => crypto.randomUUID()) }}
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
type Said = { readonly tone: "note" | "problem"; readonly words: string }

export function InstalledAppsTab() {
  const { gateway, sandbox, requestId } = useContext(InstalledAppsContext)
  const [listing, setListing] = useState<Listing>({ kind: "reading" })
  const [running, setRunning] = useState<readonly string[]>([])
  const [busy, setBusy] = useState<string>()
  const [said, setSaid] = useState<Said>()
  const [retry, setRetry] = useState(0)
  const [draft, setDraft] = useState({ appId: "", version: "1.0.0" })
  const [checked, setChecked] = useState<CheckedAppPackage>()
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

  const create = () =>
    run("create", async () => {
      if (!gateway) return
      setChecked(await gateway.starter(draft.appId.trim(), draft.version.trim()))
    })

  const install = () =>
    run("install", async () => {
      if (!gateway || !checked) return
      const { review } = checked
      const yes = await confirmAppInstall(review)
      if (!yes) {
        setSaid({ tone: "note", words: `${review.appId} wasn’t installed.` })
        return
      }
      try {
        await gateway.install(checked.package, review.digest, requestId())
      } finally {
        reread()
      }
      setChecked(undefined)
      setDraft({ appId: "", version: "1.0.0" })
      setSaid({ tone: "note", words: `${review.appId} is installed.` })
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
      // The window goes first, so no app runs from bytes the gateway removed.
      await stopInstalledApp(appId)
      try {
        await gateway.uninstall(appId, requestId())
      } finally {
        reread()
      }
      setSaid({ tone: "note", words: `${appId} is uninstalled.` })
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
  const list = setting("installed-apps")
  const make = setting("create-app")

  return (
    <>
      <Group title={list.label} note={list.detail}>
        {apps.length === 0 && (
          <ItemRow
            label={status ?? "No apps yet"}
            detail={status ? undefined : "Create one below, review it, then install it."}
            unavailable={!ready}
            control={
              listing.kind === "unread" ? (
                <RowAction onClick={reread}>Try again</RowAction>
              ) : undefined
            }
          />
        )}
        {apps.map((app) => {
          const open_ = running.includes(app.appId)
          return (
            <ItemRow
              key={app.appId}
              label={app.appId}
              detail={open_ ? `${app.packageVersion} · Running` : app.packageVersion}
              data-installed-app={app.appId}
              data-installed-app-running={open_ ? "" : undefined}
              control={
                <span className="settings-installed-app-actions">
                  {open_ ? (
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
        {said && (
          <p
            id={saidId}
            role={said.tone === "problem" ? "alert" : "status"}
            className="settings-boat-note"
            data-installed-apps-said={said.tone}
          >
            {said.words}
          </p>
        )}
      </Group>
      <Group title={make.label} note={make.detail}>
        <ItemRow
          label="New app"
          detail="Starts from a small page you can change later."
          setting="create-app"
          unavailable={!ready}
          control={
            <RowAction
              disabled={!!busy || draft.appId.trim() === ""}
              onClick={() => void create()}
              data-create-app-review=""
            >
              Review
            </RowAction>
          }
        >
          <span className="settings-terminal-rule-form">
            <Input
              className="settings-terminal-rule-field"
              type="text"
              aria-label="App id"
              placeholder="my-notes"
              data-create-app-id=""
              disabled={!ready}
              value={draft.appId}
              onChange={(event) => {
                setDraft({ ...draft, appId: event.target.value })
                setChecked(undefined)
              }}
              onKeyDown={(event) => {
                if (event.key !== "Enter") return
                event.preventDefault()
                void create()
              }}
            />
            <Input
              className="settings-terminal-rule-field"
              type="text"
              aria-label="Version"
              placeholder="1.0.0"
              data-create-app-version=""
              disabled={!ready}
              value={draft.version}
              onChange={(event) => {
                setDraft({ ...draft, version: event.target.value })
                setChecked(undefined)
              }}
            />
          </span>
        </ItemRow>
        {checked && (
          <ItemRow
            label={`Review ${checked.review.appId} ${checked.review.packageVersion}`}
            detail={`${checked.review.assetCount} ${
              checked.review.assetCount === 1 ? "file" : "files"
            }, opens ${checked.review.entrypoint}. Package ${checked.review.digest.slice(0, 12)}. It runs with no network and can reach nothing of Nessa’s.`}
            data-app-review={checked.review.digest}
            control={
              <span className="settings-installed-app-actions">
                <RowAction
                  disabled={!!busy}
                  onClick={() => setChecked(undefined)}
                  data-app-review-discard=""
                >
                  Discard
                </RowAction>
                <RowAction
                  disabled={!!busy}
                  onClick={() => void install()}
                  data-app-review-install=""
                >
                  Install
                </RowAction>
              </span>
            }
          >
            <AppPreview checked={checked} sandbox={sandbox} />
          </ItemRow>
        )}
      </Group>
    </>
  )
}

/** The reviewed package, drawn as it will run: in the sandbox, with no network. */
function AppPreview({
  checked,
  sandbox,
}: {
  checked: CheckedAppPackage
  sandbox: SandboxOrigin | undefined
}) {
  const container = useRef<HTMLDivElement>(null)
  const [unshown, setUnshown] = useState(false)
  useEffect(() => {
    const at = container.current
    if (!at || !sandbox) return
    setUnshown(false)
    let html: string
    try {
      html = packageDocument(checked.review.entrypoint, checked.package.assets, (text) =>
        new DOMParser().parseFromString(text, "text/html"),
      )
    } catch {
      setUnshown(true)
      return
    }
    // Framed on the next task, not now: an effect run only to be undone (a
    // development double run, or a review replaced at once) then frames
    // nothing, so no proxy request is started only to be aborted.
    let frame: { remove(): void } | undefined
    const later = setTimeout(() => {
      frame = mountStaticApp(at, sandbox, html, {
        title: `Preview of ${checked.review.appId}`,
      })
    }, 0)
    return () => {
      clearTimeout(later)
      frame?.remove()
    }
  }, [checked, sandbox])
  if (!sandbox || unshown)
    return (
      <p className="settings-boat-note">
        {sandbox
          ? "This app can’t be previewed."
          : "No preview here: there is no sandbox."}
      </p>
    )
  return (
    <div
      ref={container}
      className="settings-installed-app-preview"
      data-app-preview={checked.review.appId}
    />
  )
}
