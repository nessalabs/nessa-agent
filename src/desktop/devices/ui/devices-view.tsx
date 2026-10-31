/**
 * Devices, the side rail's page for the hub's machines: the hub itself, then
 * one list of every other device — the computers that run agents for it, the
 * cloud it falls back to, and the phones and apps linked to it — each saying
 * what it is underneath its name. A computer or the cloud opens its own page;
 * "Add device" asks which kind and adds it here.
 *
 * Everything shown is what the gateway last answered, read again every
 * `pollMs` while the page is open; nothing is kept once it closes.
 */
import { Button } from "@nessa-ui/react/button"
import { DeviceFigure } from "@nessa-ui/react/device-figure"
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
import { useNow } from "../../../clock"
import { linkedOn, type LinkedDevice } from "../../model/linked-device"
import { LinkedDevicesTab } from "../../settings/ui/linked-devices-tab"
import { BoatConnectionFields, useBoatConnection } from "../../settings/ui/boat-row"
import type { CloudState, DevicesGateway } from "../adapters/devices-gateway"
import {
  agentsWord,
  ago,
  hubCard,
  sortWorkers,
  statusWord,
  summary,
  workerCard,
  type DeviceStatus,
  type HubCard,
  type Usage,
  type WorkerCard,
} from "../model/devices"
import { AddComputer } from "./add-computer"
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
  readonly hub?: HubCard | null
  readonly workers?: readonly WorkerCard[]
  /** More workers than one read follows; those past it are not shown. */
  readonly more?: boolean
  /** Where the hub's native listener is bound; `null` while linking is off. */
  readonly listening?: string | null
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
        workers: list.ok ? sortWorkers(list.value.workers.map(workerCard)) : last.workers,
        more: list.ok ? list.value.more : last.more,
        listening: list.ok ? (list.value.listening ?? null) : last.listening,
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
  | { readonly kind: "worker"; readonly id: string }
  | { readonly kind: "cloud" }
  | { readonly kind: "add" }
  | { readonly kind: "add-computer" }
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
        <div className="devices-settings">
          <LinkedDevicesTab />
        </div>
      </div>
    )
  if (page.kind === "cloud") return <CloudPage onBack={list} onChanged={devices.reload} />
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
  if (page.kind === "worker") {
    const worker = devices.workers?.find((each) => each.id === page.id)
    return (
      <WorkerPage worker={worker} hub={devices.hub?.name ?? "the hub"} onBack={list} />
    )
  }
  return (
    <DevicesList
      read={devices}
      onOpen={(id) => setPage({ kind: "worker", id })}
      onCloud={() => setPage({ kind: "cloud" })}
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
  onOpen,
  onCloud,
  onAdd,
}: {
  read: Read
  onOpen: (id: string) => void
  onCloud: () => void
  onAdd: () => void
}) {
  const now = useNow(30_000)
  const loaded = read.workers !== undefined
  const hubName = read.hub?.name ?? "This Mac"
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
      {!loaded ? (
        <DelayedLoading />
      ) : (
        <>
          <HubSection hub={read.hub ?? null} />
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
  name,
  kind,
  status,
  footer,
  onClick,
  ...data
}: {
  art: WorkerCard["art"]
  name: string
  kind: string
  status: DeviceStatus
  footer: ReactNode
  onClick?: () => void
  [data: `data-${string}`]: string
}) {
  const body = (
    <>
      <div className="devices-stage">
        <DeviceFigure
          kind={art}
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
  return onClick ? (
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
}

/** Where the cloud stands, in a few words under its tile. */
function cloudWord(cloud: CloudState | null): string {
  return cloud === "connected"
    ? "Connected"
    : cloud === "locked"
      ? "Unlock the vault to use it"
      : cloud === "disconnected"
        ? "Not connected"
        : "Couldn’t read the connection"
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
      art="cloud"
      name="Boat"
      kind="Cloud VMs for channels set to run there"
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
  art: WorkerCard["art"]
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
    art: "cloud",
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
function CloudPage({ onBack, onChanged }: { onBack: () => void; onChanged: () => void }) {
  const connection = useBoatConnection(onChanged)
  const state = connection.status?.state
  return (
    <div className="devices-page" data-devices-page="cloud">
      <BackButton onBack={onBack} />
      <header className="devices-detail-head">
        <DeviceFigure
          kind="cloud"
          dimmed={state !== "connected"}
          className="devices-detail-figure"
        />
        <div className="devices-heading">
          <h1>Boat</h1>
          <p className="devices-status devices-detail-status" data-devices-cloud-state>
            <Dot status={state === "connected" ? "ready" : "offline"} />
            {state === undefined ? "Reading…" : cloudWord(state)}
            <span className="devices-muted">
              {" · "}Cloud VMs for channels set to run there
            </span>
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
      <div className="devices-settings">
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
