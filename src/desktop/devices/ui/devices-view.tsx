/**
 * Devices, the side rail's page for the hub's machines: the hub itself, then
 * one list of every other device — the clouds it is connected to, and the
 * phones and apps linked to it — each saying what it is underneath its name.
 * A cloud opens its own page; "Add device" asks which kind and adds it here.
 *
 * Every gateway runs only its own agents (ADR 1417): the hub runs every chat
 * it shows, and another computer runs a gateway of its own.
 *
 * Everything shown is what the gateway last answered, read again every
 * `pollMs` while the page is open; nothing is kept once it closes.
 */
import { Button } from "@nessa-ui/react/button"
import { DeviceFigure, type DeviceFigureKind } from "@nessa-ui/react/device-figure"
import { LoadingText } from "@nessa-ui/react/loading-text"
import { Meter } from "@nessa-ui/react/meter"
import {
  createContext,
  useContext,
  useEffect,
  useId,
  useState,
  type ReactNode,
} from "react"
import { linkedOn, type LinkedDevice } from "../../model/linked-device"
import { LinkedDevicesTab } from "../../settings/ui/linked-devices-tab"
import { PageHeader } from "@nessa-ui/react/page-header"
import { BoatConnectionFields, useBoatConnection } from "../../settings/ui/boat-row"
import { Agent37Row } from "../../settings/ui/agent37-row"
import { clouds, cloudProvider, type Cloud } from "../../clouds/model/clouds"
import { CloudFigure } from "../../clouds/ui/cloud-figure"
import { useCloudStates } from "../../clouds/ui/cloud-states"
import type { CloudState, DevicesGateway } from "../adapters/devices-gateway"
import {
  hubCard,
  statusWord,
  type DeviceStatus,
  type HubCard,
  type Usage,
} from "../model/devices"
import "./devices.css"

const Gateway = createContext<DevicesGateway | undefined>(undefined)

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
  /** `undefined` until the first answer; `null` when the hub cannot be read. */
  readonly hub?: HubCard | null
  readonly cloud?: CloudState | null
  readonly linked?: readonly LinkedDevice[] | null
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
      const [list, cloud, linked] = await Promise.all([
        gateway.list(),
        gateway.cloud(),
        gateway.linked(),
      ])
      if (!live) return
      setRead((last) => ({
        // A list that could not read the hub this time keeps the last one.
        hub: list.ok && list.value.hub ? hubCard(list.value.hub) : (last.hub ?? null),
        cloud: cloud.ok ? cloud.value : null,
        linked: linked.ok ? linked.value : null,
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
  | { readonly kind: "cloud"; readonly cloud: Cloud }
  | { readonly kind: "add" }
  | { readonly kind: "add-cloud" }
  | { readonly kind: "add-phone" }

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
        onPick={(kind) => setPage({ kind: kind === "phone" ? "add-phone" : "add-cloud" })}
      />
    )
  if (page.kind === "add-cloud")
    return (
      <AddCloud
        onBack={() => setPage({ kind: "add" })}
        onPick={(cloud) => setPage({ kind: "cloud", cloud })}
      />
    )
  if (page.kind === "add-phone")
    return (
      <div className="devices-page devices-add-page" data-devices-page="add-phone">
        <BackButton onBack={list} />
        <div className="devices-heading">
          <PageHeader title="Add a phone" />
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
    return page.cloud === "boat" ? (
      <CloudPage cloud={devices.cloud} onBack={list} onChanged={devices.reload} />
    ) : (
      <Agent37Page onBack={list} />
    )
  return (
    <DevicesList
      read={devices}
      onCloud={(cloud) => setPage({ kind: "cloud", cloud })}
      onAdd={() => setPage({ kind: "add" })}
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
  onCloud,
  onAdd,
}: {
  read: Read
  onCloud: (cloud: Cloud) => void
  onAdd: () => void
}) {
  const loaded = read.hub !== undefined
  return (
    <div className="devices-page" data-devices-page="list">
      <PageHeader
        title="Devices"
        actions={
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
        }
      />
      {read.unreachable ? (
        <p className="devices-muted" role="status" data-devices-unreachable>
          The hub isn&apos;t answering. Showing what it last said.
        </p>
      ) : null}
      {!loaded ? (
        <DelayedLoading />
      ) : (
        <>
          <HubSection hub={read.hub ?? null} />
          <section className="devices-section" aria-label="Your devices">
            <div className="devices-grid" data-devices-list>
              <CloudTiles boat={read.cloud} seen={read} onOpen={onCloud} />
              <LinkedTiles linked={read.linked} />
            </div>
          </section>
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

function HubSection({ hub }: { hub: HubCard | null }) {
  return (
    <section className="devices-hub" aria-labelledby="devices-hub-name" data-devices-hub>
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
  figure,
  afloat,
  name,
  kind,
  status,
  said = statusWord(status),
  footer,
  onClick,
  ...data
}: {
  art?: DeviceFigureKind
  /** Drawn in place of `art`: a picture of its own, such as a cloud's mark. */
  figure?: ReactNode
  /** A boat bobs on its water while it is not dimmed. */
  afloat?: boolean
  name: string
  kind: string
  status: DeviceStatus
  /** The status in words, when the device's own word says more than `status`'s. */
  said?: string
  footer?: ReactNode
  onClick?: () => void
  [data: `data-${string}`]: string
}) {
  const body = (
    <>
      <div className="devices-stage">
        {figure ?? (
          <DeviceFigure
            kind={art ?? "server"}
            afloat={afloat}
            dimmed={status === "offline"}
            className="devices-tile-figure"
          />
        )}
      </div>
      <div className="devices-tile-body">
        <div className="devices-tile-row">
          <span className="devices-tile-name">{name}</span>
          <span className="devices-status">
            <Dot status={status} />
            {said}
          </span>
        </div>
        <span className="devices-muted">{kind}</span>
        {footer !== undefined ? <div className="devices-tile-foot">{footer}</div> : null}
      </div>
    </>
  )
  return onClick ? (
    <button
      type="button"
      className="devices-tile"
      onClick={onClick}
      aria-label={`${name}, ${said}`}
      {...data}
    >
      {body}
    </button>
  ) : (
    <div className="devices-tile" {...data}>
      {body}
    </div>
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

/**
 * A tile for each cloud: Boat's state comes with the list's own read; each
 * other cloud's is read beside it, again on each of the list's reads.
 */
function CloudTiles({
  boat,
  seen,
  onOpen,
}: {
  boat: CloudState | null | undefined
  seen: unknown
  onOpen: (cloud: Cloud) => void
}) {
  const states = useCloudStates(seen)
  return clouds.map(({ id, name, kind }) => {
    const state = id === "boat" ? boat : states[id]
    if (state === undefined) return null
    return (
      <Tile
        key={id}
        figure={
          <CloudFigure
            cloud={id}
            dimmed={state !== "connected"}
            className="devices-tile-figure"
          />
        }
        name={name}
        kind={kind}
        status={state === "connected" ? "ready" : "offline"}
        // One word: where the cloud stands, said once beside its name.
        said={cloudWord(state)}
        onClick={() => onOpen(id)}
        data-devices-cloud={state ?? "unknown"}
        data-devices-cloud-id={id}
      />
    )
  })
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

type AddKind = "phone" | "cloud"

const addKinds: readonly {
  kind: AddKind
  art: DeviceFigureKind
  name: string
  detail: string
}[] = [
  { kind: "phone", art: "phone", name: "Phone", detail: "Talk to the hub from anywhere" },
  {
    kind: "cloud",
    art: "boat",
    name: "Cloud",
    detail: `${clouds.map((cloud) => cloud.name).join(" or ")} accounts for cloud boxes`,
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
        <PageHeader title="Add device" />
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

/** "Add device › Cloud": which cloud, then its own page to connect it. */
function AddCloud({
  onBack,
  onPick,
}: {
  onBack: () => void
  onPick: (cloud: Cloud) => void
}) {
  return (
    <div className="devices-page" data-devices-page="add-cloud">
      <BackButton onBack={onBack} />
      <div className="devices-heading">
        <PageHeader title="Add a cloud" />
        <p className="devices-muted">Which cloud should Nessa connect to?</p>
      </div>
      <div className="devices-grid">
        {clouds.map((cloud) => (
          <button
            key={cloud.id}
            type="button"
            className="devices-tile"
            onClick={() => onPick(cloud.id)}
            data-devices-add-cloud={cloud.id}
          >
            <div className="devices-stage">
              <CloudFigure cloud={cloud.id} className="devices-tile-figure" />
            </div>
            <div className="devices-tile-body">
              <span className="devices-tile-name">{cloud.name}</span>
              <span className="devices-muted">{cloud.detail}</span>
            </div>
          </button>
        ))}
      </div>
    </div>
  )
}

/** Agent37's own page: its key. */
function Agent37Page({ onBack }: { onBack: () => void }) {
  // The row below reads, connects, unlocks and disconnects; the header
  // follows what it last read.
  const [read, setRead] = useState<CloudState | undefined>()
  const states = useCloudStates()
  const state = read ?? states.agent37
  const word = state === undefined ? "Reading…" : cloudWord(state)
  return (
    <div
      className="devices-page"
      data-devices-page="cloud"
      data-devices-cloud-page="agent37"
    >
      <BackButton onBack={onBack} />
      <header className="devices-detail-head">
        <CloudFigure
          cloud="agent37"
          dimmed={state !== "connected"}
          className="devices-detail-figure"
        />
        <div className="devices-heading">
          <PageHeader title={cloudProvider("agent37").name} />
          <p
            className="devices-status devices-detail-status"
            data-devices-cloud-state={word}
          >
            <Dot status={state === "connected" ? "ready" : "offline"} />
            {word}
            <span className="devices-muted">{" · "}Cloud VMs</span>
          </p>
        </div>
      </header>
      <div className="devices-settings settings-surface">
        <Agent37Row onChange={setRead} />
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
        <CloudFigure
          cloud="boat"
          dimmed={state !== "connected"}
          className="devices-detail-figure"
        />
        <div className="devices-heading">
          <PageHeader title="Boat" />
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
