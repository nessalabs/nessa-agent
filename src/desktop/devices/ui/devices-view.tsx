/**
 * Devices, the side rail's page for the hub's machines: the hub itself, then
 * one list of every other device — the computers that run agents for it,
 * the SSH hosts it runs them on, the cloud it falls back to, and the phones
 * and apps linked to it — each saying what it is underneath its name. A
 * computer or the cloud opens its own page; "Add device" asks which kind
 * and adds it here.
 *
 * A star on the hub, a computer or an SSH host makes it the default device:
 * where a new chat runs, from this window or a phone, when it names none.
 * Under the list, the machines the hub's `~/.ssh/config` names are
 * suggested; Connect on one installs Nessa there over SSH and adds it, and
 * the hub starts again to serve it.
 *
 * Everything shown is what the gateway last answered, read again every
 * `pollMs` while the page is open; nothing is kept once it closes.
 */
import { Button } from "@nessa-ui/react/button"
import { DeviceFigure, type DeviceFigureKind } from "@nessa-ui/react/device-figure"
import { LoadingText } from "@nessa-ui/react/loading-text"
import { Meter } from "@nessa-ui/react/meter"
import { Star } from "lucide-react"
import {
  createContext,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { useNow } from "../../../clock"
import { linkedOn, type LinkedDevice } from "../../model/linked-device"
import { LinkedDevicesTab } from "../../settings/ui/linked-devices-tab"
import { BoatConnectionFields, useBoatConnection } from "../../settings/ui/boat-row"
import type {
  CloudState,
  DevicesGateway,
  SshSuggestions,
} from "../adapters/devices-gateway"
import {
  agentsWord,
  ago,
  connectWord,
  hubCard,
  sshHostCard,
  sortWorkers,
  statusWord,
  summary,
  workerCard,
  type DeviceStatus,
  type DeviceSshConnect,
  type HubCard,
  type SshHostCard,
  type Usage,
  type WorkerCard,
} from "../model/devices"
import { AddComputer } from "./add-computer"
import { PoolOrder } from "./pool-order"
import "./devices.css"

const Gateway = createContext<DevicesGateway | undefined>(undefined)

/** The window's devices gateway, for a channel's thread order. */
export function useDevicesGateway(): DevicesGateway | undefined {
  return useContext(Gateway)
}

export function DevicesProvider({
  gateway,
  children,
}: {
  gateway: DevicesGateway | undefined
  children: ReactNode
}) {
  return <Gateway.Provider value={gateway}>{children}</Gateway.Provider>
}

/** What the page last read. `undefined`: not answered yet. */
interface Read {
  readonly hub?: HubCard | null
  readonly workers?: readonly WorkerCard[]
  /** More workers than one read follows; those past it are not shown. */
  readonly more?: boolean
  /** Where the hub's native listener is bound; `null` while linking is off. */
  readonly listening?: string | null
  readonly cloud?: CloudState | null
  readonly linked?: readonly LinkedDevice[] | null
  /** The SSH hosts agents run on. */
  readonly sshHosts?: readonly SshHostCard[]
  /** Where a new chat runs when it names none; `null` is the hub. */
  readonly defaultEnvironment?: string | null
  /** Machines to suggest, and each setup under way; `null` unread. */
  readonly suggestions?: SshSuggestions | null
  /** The last read of the list failed. */
  readonly unreachable: boolean
}

/** Reads the devices now and every `pollMs` until unmounted. */
function useDevices(gateway: DevicesGateway | undefined): Read & { reload: () => void } {
  const [read, setRead] = useState<Read>({ unreachable: false })
  const [round, setRound] = useState(0)
  useEffect(() => {
    if (!gateway) return
    let live = true
    const load = async () => {
      const [list, cloud, linked, suggestions] = await Promise.all([
        gateway.list(),
        gateway.cloud(),
        gateway.linked(),
        gateway.sshHosts(),
      ])
      if (!live) return
      setRead((last) => ({
        // A list that could not read the hub this time keeps the last one.
        hub: list.ok && list.value.hub ? hubCard(list.value.hub) : (last.hub ?? null),
        workers: list.ok ? sortWorkers(list.value.workers.map(workerCard)) : last.workers,
        more: list.ok ? list.value.more : last.more,
        listening: list.ok ? (list.value.listening ?? null) : last.listening,
        cloud: cloud.ok ? cloud.value : null,
        linked: linked.ok ? linked.value : null,
        sshHosts: list.ok ? list.value.sshHosts.map(sshHostCard) : last.sshHosts,
        defaultEnvironment: list.ok
          ? list.value.defaultEnvironment
          : last.defaultEnvironment,
        suggestions: suggestions.ok ? suggestions.value : (last.suggestions ?? null),
        unreachable: !list.ok,
      }))
    }
    void load()
    const timer = window.setInterval(() => void load(), gateway.pollMs)
    return () => {
      live = false
      window.clearInterval(timer)
    }
  }, [gateway, round])
  return { ...read, reload: () => setRound((value) => value + 1) }
}

type Page =
  | { readonly kind: "list" }
  | { readonly kind: "worker"; readonly id: string }
  | { readonly kind: "cloud" }
  | { readonly kind: "add" }
  | { readonly kind: "add-computer" }
  | { readonly kind: "add-phone" }
  | { readonly kind: "pool" }

export function DevicesView() {
  const gateway = useContext(Gateway)
  const devices = useDevices(gateway)
  const [page, setPage] = useState<Page>({ kind: "list" })
  if (!gateway)
    return (
      <div className="devices-page" data-devices-page="none">
        <p className="devices-muted">
          Devices show once Nessa is connected to its gateway.
        </p>
      </div>
    )
  const list = () => setPage({ kind: "list" })
  if (page.kind === "add")
    return (
      <AddDevice
        onBack={list}
        onPick={(kind) =>
          setPage({
            kind:
              kind === "computer"
                ? "add-computer"
                : kind === "phone"
                  ? "add-phone"
                  : "cloud",
          })
        }
      />
    )
  if (page.kind === "add-phone")
    return (
      <div className="devices-page devices-add-page" data-devices-page="add-phone">
        <BackButton onBack={list} />
        <div className="devices-heading">
          <h1>Add a phone</h1>
          <p className="devices-muted">
            Pair the Nessa app to talk to the hub from anywhere.
          </p>
        </div>
        <div className="devices-settings settings-surface">
          <LinkedDevicesTab />
        </div>
      </div>
    )
  if (page.kind === "cloud")
    return <CloudPage cloud={devices.cloud} onBack={list} onChanged={devices.reload} />
  if (page.kind === "add-computer")
    return (
      <AddComputer
        gateway={gateway}
        hubName={devices.hub?.name}
        listening={devices.listening}
        onReached={devices.reload}
        onDone={() => {
          devices.reload()
          setPage({ kind: "list" })
        }}
      />
    )
  if (page.kind === "pool")
    return (
      <div className="devices-page" data-devices-page="pool">
        <BackButton onBack={list} />
        <div className="devices-heading">
          <h1>Thread order</h1>
          <p className="devices-muted">
            Which computer a new thread on Any computer runs on. A channel can keep its
            own order in its settings.
          </p>
        </div>
        <div className="devices-settings settings-surface">
          <PoolOrder gateway={gateway} workers={devices.workers} />
        </div>
      </div>
    )
  if (page.kind === "worker") {
    const worker = devices.workers?.find((each) => each.id === page.id)
    return (
      <WorkerPage worker={worker} hub={devices.hub?.name ?? "the hub"} onBack={list} />
    )
  }
  return (
    <DevicesList
      read={devices}
      gateway={gateway}
      onChanged={devices.reload}
      onOpen={(id) => setPage({ kind: "worker", id })}
      onCloud={() => setPage({ kind: "cloud" })}
      onAdd={() => setPage({ kind: "add" })}
      onPool={() => setPage({ kind: "pool" })}
    />
  )
}

const plus = (
  <svg
    viewBox="0 0 16 16"
    width="14"
    height="14"
    aria-hidden="true"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.8"
    strokeLinecap="round"
  >
    <path d="M8 3v10M3 8h10" />
  </svg>
)

function DevicesList({
  read,
  gateway,
  onChanged,
  onOpen,
  onCloud,
  onAdd,
  onPool,
}: {
  read: Read
  gateway: DevicesGateway
  /** Something was saved: read the list again. */
  onChanged: () => void
  onOpen: (id: string) => void
  onCloud: () => void
  onAdd: () => void
  onPool: () => void
}) {
  const now = useNow(30_000)
  const loaded = read.workers !== undefined
  const hubName = read.hub?.name ?? "This Mac"
  // The star a person last pressed, shown at once and until its answer;
  // an earlier press answering later does not take it back.
  const [starring, setStarring] = useState<{
    readonly to: string | null
    readonly press: number
  } | null>(null)
  const presses = useRef(0)
  const [starFailed, setStarFailed] = useState(false)
  const current = starring !== null ? starring.to : (read.defaultEnvironment ?? null)
  const makeDefault = (environment: string | null) => {
    const press = (presses.current += 1)
    setStarring({ to: environment, press })
    setStarFailed(false)
    void gateway.setDefault(environment).then((answer) => {
      if (presses.current !== press) return
      setStarring(null)
      setStarFailed(!answer.ok)
      onChanged()
    })
  }
  const star = (environment: string | null, name: string) =>
    loaded && read.defaultEnvironment !== undefined ? (
      <DefaultStar
        environment={environment}
        current={current}
        name={name}
        onSet={makeDefault}
      />
    ) : null
  return (
    <div className="devices-page" data-devices-page="list">
      <header className="devices-header">
        <div className="devices-heading">
          <h1>Devices</h1>
          <p className="devices-muted" data-devices-summary>
            {loaded ? summary(hubName, read.workers ?? []) : " "}
          </p>
        </div>
        <Button
          className="devices-primary"
          variant="inverse"
          size="30"
          shape="pill"
          onClick={onAdd}
          data-devices-action="add"
        >
          {plus}
          Add device
        </Button>
      </header>
      {read.unreachable ? (
        <p className="devices-muted" role="status" data-devices-unreachable>
          The hub isn&apos;t answering. Showing what it last said.
        </p>
      ) : null}
      {starFailed ? (
        <p className="devices-muted" role="status" data-devices-default-failed>
          Couldn&apos;t change where new chats run. Try again.
        </p>
      ) : null}
      {!loaded ? (
        <DelayedLoading />
      ) : (
        <>
          <HubSection hub={read.hub ?? null} star={star(null, hubName)} />
          <section className="devices-section" aria-label="Your devices">
            <div className="devices-grid" data-devices-list>
              {(read.workers ?? []).map((worker) => (
                <Tile
                  key={worker.id}
                  art={worker.art}
                  name={worker.name}
                  kind={worker.system}
                  status={worker.status}
                  onClick={() => onOpen(worker.id)}
                  star={star(worker.environment, worker.name)}
                  data-devices-worker={worker.id}
                  footer={
                    worker.status === "offline" ? (
                      <span>
                        {worker.lastSeenMs !== undefined
                          ? `Last seen ${ago(worker.lastSeenMs, now)}`
                          : "Never seen"}
                      </span>
                    ) : (
                      <span>
                        {agentsWord(worker.agents)}
                        {worker.cpu ? ` · CPU ${worker.cpu.value}` : ""}
                      </span>
                    )
                  }
                />
              ))}
              {(read.sshHosts ?? []).map((host) => (
                <Tile
                  key={host.host}
                  art="server"
                  name={host.name}
                  kind={host.kind}
                  status="ready"
                  star={star(host.host, host.name)}
                  data-devices-ssh={host.host}
                  footer={<span>{host.address}</span>}
                />
              ))}
              <CloudTile cloud={read.cloud} onOpen={onCloud} />
              <LinkedTiles linked={read.linked} />
              <button
                type="button"
                className="devices-tile devices-tile-add"
                onClick={onAdd}
                data-devices-action="add-tile"
              >
                {plus}
                <span>Add device</span>
              </button>
            </div>
            {read.more ? (
              <p className="devices-muted" data-devices-more>
                More computers than these work for the hub; they are not shown.
              </p>
            ) : null}
            {read.workers?.length ? (
              <button
                type="button"
                className="devices-pool-link"
                onClick={onPool}
                data-devices-action="pool"
              >
                <span>Thread order</span>
                <span className="devices-muted">
                  Which computer new threads run on first
                </span>
              </button>
            ) : null}
          </section>
          <Suggestions
            suggestions={read.suggestions}
            gateway={gateway}
            onChanged={onChanged}
          />
        </>
      )}
    </div>
  )
}

/** Nothing for a moment, then the kit's loading text: most reads land first. */
function DelayedLoading() {
  const [long, setLong] = useState(false)
  useEffect(() => {
    const timer = window.setTimeout(() => setLong(true), 3000)
    return () => window.clearTimeout(timer)
  }, [])
  return long ? (
    <LoadingText className="rail-view-loading devices-loading">
      Loading Devices
    </LoadingText>
  ) : null
}

function Dot({ status }: { status: DeviceStatus }) {
  return <i className="devices-dot" data-status={status} aria-hidden="true" />
}

function HubSection({ hub, star }: { hub: HubCard | null; star: ReactNode }) {
  return (
    <section className="devices-hub" aria-labelledby="devices-hub-name" data-devices-hub>
      {star ? <div className="devices-star-slot">{star}</div> : null}
      <DeviceFigure kind="desktop" className="devices-hub-figure" />
      <div className="devices-hub-title">
        <h2 id="devices-hub-name">
          {hub?.name ?? "This Mac"}
          <Dot status="online" />
        </h2>
        <p className="devices-muted">{hub ? `${hub.system} · your hub` : "Your hub"}</p>
      </div>
      {hub ? (
        <div className="devices-stats">
          <Stat label="CPU" usage={hub.cpu} />
          <Stat label="Memory" usage={hub.memory} />
          <Stat label="Disk" usage={hub.disk} />
        </div>
      ) : null}
    </section>
  )
}

function Stat({ label, usage }: { label: string; usage: Usage }) {
  const id = useId()
  return (
    <div className="devices-stat" data-devices-stat={label}>
      <span className="devices-stat-label" id={id} title={label}>
        {label}
      </span>
      <span className="devices-stat-value">
        {usage.value}
        {usage.of ? <span className="devices-muted"> {usage.of}</span> : null}
      </span>
      <Meter
        value={usage.fraction}
        aria-labelledby={id}
        valueText={`${usage.value} ${usage.of}`.trim()}
      />
    </div>
  )
}

function Tile({
  art,
  afloat,
  name,
  kind,
  status,
  footer,
  onClick,
  star,
  ...data
}: {
  art: DeviceFigureKind
  /** A boat bobs on its water while it is not dimmed. */
  afloat?: boolean
  name: string
  kind: string
  status: DeviceStatus
  footer: ReactNode
  onClick?: () => void
  /** Makes it the default device; beside the tile, never inside its button. */
  star?: ReactNode
  [data: `data-${string}`]: string
}) {
  const body = (
    <>
      <div className="devices-stage">
        <DeviceFigure
          kind={art}
          afloat={afloat}
          dimmed={status === "offline"}
          className="devices-tile-figure"
        />
      </div>
      <div className="devices-tile-body">
        <div className="devices-tile-row">
          <span className="devices-tile-name">{name}</span>
          <span className="devices-status">
            <Dot status={status} />
            {statusWord(status)}
          </span>
        </div>
        <span className="devices-muted">{kind}</span>
        <div className="devices-tile-foot">{footer}</div>
      </div>
    </>
  )
  const tile = onClick ? (
    <button
      type="button"
      className="devices-tile"
      onClick={onClick}
      aria-label={`${name}, ${statusWord(status)}`}
      {...data}
    >
      {body}
    </button>
  ) : (
    <div className="devices-tile" {...data}>
      {body}
    </div>
  )
  return star ? (
    <div className="devices-tile-holder">
      {tile}
      <div className="devices-star-slot">{star}</div>
    </div>
  ) : (
    tile
  )
}

/** The star that makes a device where new chats run: filled on that one. */
function DefaultStar({
  environment,
  current,
  name,
  onSet,
}: {
  /** The device: `null` is the hub. */
  environment: string | null
  current: string | null
  name: string
  onSet: (environment: string | null) => void
}) {
  const chosen = environment === current
  return (
    <Button
      variant="plain"
      size="icon-sm"
      shape="pill"
      className="devices-star"
      aria-pressed={chosen}
      aria-label={chosen ? `${name} is where new chats run` : `Run new chats on ${name}`}
      title={chosen ? "Default device: new chats run here" : "Make default"}
      onClick={() => {
        if (!chosen) onSet(environment)
      }}
      data-devices-default={environment ?? "hub"}
      data-chosen={chosen ? "" : undefined}
    >
      <Star aria-hidden="true" fill={chosen ? "currentColor" : "none"} />
    </Button>
  )
}

/**
 * The machines the hub's SSH configuration names, as pills under the list:
 * each shows Connect on hover or focus, and then how its setup goes. One
 * that restarted the hub leaves this list for the one above.
 */
function Suggestions({
  suggestions,
  gateway,
  onChanged,
}: {
  suggestions: SshSuggestions | null | undefined
  gateway: DevicesGateway
  onChanged: () => void
}) {
  // A Connect pressed shows where it stands until the gateway's read that
  // follows its answer says, and no longer: a read after a restart no
  // longer names it, and an old answer would hold the pill on Installing.
  const [pressed, setPressed] = useState<
    Readonly<Record<string, { connect: DeviceSshConnect; answered: boolean }>>
  >({})
  useEffect(() => {
    setPressed((last) => {
      const kept = Object.entries(last).filter(([, each]) => !each.answered)
      return kept.length === Object.keys(last).length ? last : Object.fromEntries(kept)
    })
  }, [suggestions])
  if (!suggestions) return null
  const under = new Map(suggestions.connecting.map((each) => [each.host, each]))
  const stateOf = (host: string) => under.get(host) ?? pressed[host]?.connect
  const shown = suggestions.suggested.filter(
    (host) => stateOf(host.host)?.state !== "connected",
  )
  if (shown.length === 0) return null
  const connect = (host: string) => {
    setPressed((last) => ({
      ...last,
      [host]: { connect: { host, state: "installing" }, answered: false },
    }))
    void gateway.connectSsh(host).then((answer) => {
      setPressed((last) => ({
        ...last,
        // Not answered at all: said as a setup that failed, never held on
        // Installing.
        [host]: {
          connect: answer.ok
            ? answer.value
            : { host, state: "failed", refusal: "devices_unavailable" },
          answered: answer.ok,
        },
      }))
      onChanged()
    })
  }
  return (
    <section
      className="devices-section devices-suggestions"
      aria-labelledby="devices-suggestions-title"
      data-devices-suggestions
    >
      <div className="devices-suggestions-head">
        <h2 id="devices-suggestions-title">From your SSH config</h2>
        <p className="devices-muted">
          Connect one to install Nessa there and run agents on it.
        </p>
      </div>
      <ul className="devices-pills">
        {shown.map((host) => {
          const card = sshHostCard(host)
          const state = stateOf(host.host)
          const busy = state?.state === "installing" || state?.state === "restarting"
          return (
            <li
              key={host.host}
              className="devices-pill"
              data-devices-suggested={host.host}
              data-state={state?.state ?? "idle"}
            >
              <span className="devices-pill-name">{host.host}</span>
              {state ? (
                <span className="devices-pill-note" role="status">
                  {connectWord(state)}
                </span>
              ) : (
                <span className="devices-pill-note devices-muted">{card.address}</span>
              )}
              {busy ? null : (
                <Button
                  className="devices-pill-connect"
                  variant="inverse"
                  size="24"
                  shape="pill"
                  onClick={() => connect(host.host)}
                  aria-label={`Connect ${host.host}`}
                  data-devices-action="connect"
                >
                  {state?.state === "failed" ? "Try again" : "Connect"}
                </Button>
              )}
            </li>
          )
        })}
      </ul>
    </section>
  )
}

/** Where the cloud stands, in a word or two: what to do about it is on its page. */
function cloudWord(cloud: CloudState | null): string {
  return cloud === "connected"
    ? "Connected"
    : cloud === "locked"
      ? "Locked"
      : cloud === null
        ? "Unavailable"
        : "Not connected"
}

function CloudTile({
  cloud,
  onOpen,
}: {
  cloud: CloudState | null | undefined
  onOpen: () => void
}) {
  if (cloud === undefined) return null
  return (
    <Tile
      art="boat"
      afloat
      name="Boat"
      kind="Cloud VMs"
      status={cloud === "connected" ? "ready" : "offline"}
      onClick={onOpen}
      data-devices-cloud={cloud ?? "unknown"}
      footer={<span>{cloudWord(cloud)}</span>}
    />
  )
}

function LinkedTiles({ linked }: { linked: readonly LinkedDevice[] | null | undefined }) {
  if (!linked) return null
  return linked.map((device) => (
    <Tile
      key={device.credentialId}
      art="phone"
      name={device.fullAccess ? "Your phone" : "Linked device"}
      kind={device.fullAccess ? "Nessa app · full access" : "Reads shared chats"}
      status="online"
      data-devices-linked={device.credentialId}
      footer={<span>Linked {linkedOn(device.issuedAt)}</span>}
    />
  ))
}

type AddKind = "computer" | "phone" | "cloud"

const addKinds: readonly {
  kind: AddKind
  art: DeviceFigureKind
  name: string
  detail: string
}[] = [
  {
    kind: "computer",
    art: "laptop",
    name: "Computer",
    detail: "Runs agents the hub sends it",
  },
  { kind: "phone", art: "phone", name: "Phone", detail: "Talk to the hub from anywhere" },
  {
    kind: "cloud",
    art: "boat",
    name: "Cloud",
    detail: "Boat VMs for channels to run in",
  },
]

/** "Add device": which kind, then that kind's own way in. */
function AddDevice({
  onBack,
  onPick,
}: {
  onBack: () => void
  onPick: (kind: AddKind) => void
}) {
  return (
    <div className="devices-page" data-devices-page="add-device">
      <BackButton onBack={onBack} />
      <div className="devices-heading">
        <h1>Add device</h1>
        <p className="devices-muted">What would you like to add?</p>
      </div>
      <div className="devices-grid">
        {addKinds.map((each) => (
          <button
            key={each.kind}
            type="button"
            className="devices-tile"
            onClick={() => onPick(each.kind)}
            data-devices-add={each.kind}
          >
            <div className="devices-stage">
              <DeviceFigure kind={each.art} className="devices-tile-figure" />
            </div>
            <div className="devices-tile-body">
              <span className="devices-tile-name">{each.name}</span>
              <span className="devices-muted">{each.detail}</span>
            </div>
          </button>
        ))}
      </div>
    </div>
  )
}

/** Boat's own page: its connection and VM settings, managed here. */
function CloudPage({
  cloud,
  onBack,
  onChanged,
}: {
  cloud: CloudState | null | undefined
  onBack: () => void
  onChanged: () => void
}) {
  // The list's poll is what notices the vault opening elsewhere: a new
  // answer there reads Boat's status here again.
  const connection = useBoatConnection(onChanged, cloud)
  const state = connection.status?.state
  const word =
    state !== undefined
      ? cloudWord(state)
      : connection.unread
        ? cloudWord(null)
        : "Reading…"
  return (
    <div className="devices-page" data-devices-page="cloud">
      <BackButton onBack={onBack} />
      <header className="devices-detail-head">
        <DeviceFigure
          kind="boat"
          afloat
          dimmed={state !== "connected"}
          className="devices-detail-figure"
        />
        <div className="devices-heading">
          <h1>Boat</h1>
          <p
            className="devices-status devices-detail-status"
            data-devices-cloud-state={word}
          >
            <Dot status={state === "connected" ? "ready" : "offline"} />
            {word}
            <span className="devices-muted">{" · "}Cloud VMs</span>
          </p>
        </div>
        {state === "connected" ? (
          <Button
            className="devices-detail-action"
            variant="danger"
            size="30"
            shape="pill"
            disabled={connection.busy}
            onClick={() => void connection.disconnect()}
            data-devices-action="disconnect"
          >
            Disconnect
          </Button>
        ) : null}
      </header>
      <div className="devices-settings settings-surface">
        <BoatConnectionFields connection={connection} />
      </div>
    </div>
  )
}

function BackButton({ onBack }: { onBack: () => void }) {
  return (
    <button
      type="button"
      className="devices-back"
      onClick={onBack}
      data-devices-action="back"
    >
      {back}
      Devices
    </button>
  )
}

const back = (
  <svg
    viewBox="0 0 16 16"
    width="14"
    height="14"
    aria-hidden="true"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.8"
    strokeLinecap="round"
    strokeLinejoin="round"
  >
    <path d="M10 3 5 8l5 5" />
  </svg>
)

/** A worker's own page: what it is and how busy, never what runs on it (the Agents panel says that). */
function WorkerPage({
  worker,
  hub,
  onBack,
}: {
  worker: WorkerCard | undefined
  hub: string
  onBack: () => void
}) {
  const now = useNow(30_000)
  return (
    <div className="devices-page" data-devices-page="worker">
      <BackButton onBack={onBack} />
      {!worker ? (
        <p className="devices-muted">This computer is no longer listed.</p>
      ) : (
        <>
          <header className="devices-detail-head">
            <DeviceFigure
              kind={worker.art}
              dimmed={worker.status === "offline"}
              className="devices-detail-figure"
            />
            <div className="devices-heading">
              <h1>{worker.name}</h1>
              <p className="devices-status devices-detail-status">
                <Dot status={worker.status} />
                {statusWord(worker.status)}
                <span className="devices-muted">
                  {" · "}
                  {worker.status === "offline"
                    ? worker.lastSeenMs !== undefined
                      ? `last seen ${ago(worker.lastSeenMs, now)}`
                      : "never seen"
                    : `works for ${hub}`}
                </span>
              </p>
            </div>
          </header>
          {worker.cpu && worker.memory && worker.disk ? (
            <div className="devices-stats devices-detail-stats">
              <div className="devices-stat" data-devices-stat="Agents">
                <span className="devices-stat-label">Agents</span>
                <span className="devices-stat-value">{worker.agents}</span>
              </div>
              <Stat label="CPU" usage={worker.cpu} />
              <Stat label="Memory" usage={worker.memory} />
              <Stat label="Disk" usage={worker.disk} />
            </div>
          ) : null}
          {worker.disks ? (
            <section className="devices-disks" aria-label="Disks" data-devices-disks>
              <h2>Disks</h2>
              <div className="devices-stats">
                {worker.disks.map((disk, index) => (
                  // Mounts are cut to 128 bytes, so two may read the same.
                  <Stat
                    key={`${index}:${disk.mount}`}
                    label={disk.mount}
                    usage={disk.usage}
                  />
                ))}
              </div>
            </section>
          ) : null}
          <dl className="devices-facts">
            <div>
              <dt>System</dt>
              <dd>{worker.system}</dd>
            </div>
            {worker.cpus !== undefined ? (
              <div>
                <dt>Processors</dt>
                <dd>{worker.cpus}</dd>
              </div>
            ) : null}
            <div>
              <dt>Takes new work</dt>
              <dd>
                {worker.status === "online"
                  ? "Yes"
                  : worker.status === "draining"
                    ? "No, finishing what it has"
                    : "No, it is offline"}
              </dd>
            </div>
            <div>
              <dt>Environment</dt>
              <dd className="devices-mono" title={worker.environment}>
                {`peer:${worker.id.slice(0, 12)}…`}
              </dd>
            </div>
          </dl>
        </>
      )}
    </div>
  )
}
