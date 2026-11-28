import { useId, useRef, useState } from "react"
import { Dialog } from "@nessa-ui/react/dialog"
import { IconButton } from "../../../ui/icon-button"
import { useAgent37Connection } from "../../../settings/ui/agent37-row"
import { useBoatConnection } from "../../../settings/ui/boat-row"
import {
  ReplyStyleControl,
  useReplyStyleGateway,
} from "../../../settings/ui/reply-style-control"
import { Choice, ItemRow, RowAction } from "../../../settings/ui/settings-controls"
import { channelFailure } from "../../application/ports"
import { gatewayChannelId } from "../../adapters/gateway/gateway-views"
import { setChannelCoordinator } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannel, selectGateway } from "../../adapters/store/selectors"
import { LOCAL_GATEWAY_ID } from "../../model/gateway-id"
import "./channel-settings.css"

/**
 * Whether a made channel has a coordinator, switched at any time. Off keeps
 * its chat and its threads; the channel just opens without it. On again,
 * the same chat is its coordinator. Saves as it is chosen.
 */
export function ChannelCoordinatorControl({ channelId }: { channelId: string }) {
  const dispatch = useWorkspaceDispatch()
  const channel = useWorkspaceSelector((state) => selectChannel(state, channelId))
  const [saving, setSaving] = useState(false)
  const [failed, setFailed] = useState<string | null>(null)
  if (!channel || channel.coordinatorOn === undefined) return null
  const on = channel.coordinatorOn
  const change = (next: "on" | "off") => {
    if (saving || (next === "on") === on) return
    setSaving(true)
    setFailed(null)
    void dispatch(setChannelCoordinator({ channelId, on: next === "on" }))
      .then((changed) => {
        if (!changed) setFailed("This window cannot change this channel.")
      })
      .catch((error: unknown) => {
        setFailed(
          channelFailure(error) === "forbidden"
            ? "You can’t change this channel."
            : "Couldn’t save that. Try again in a moment.",
        )
      })
      .finally(() => setSaving(false))
  }
  return (
    <div
      role="group"
      aria-label="Coordinator"
      aria-busy={saving || undefined}
      data-channel-coordinator={channelId}
      data-coordinator-on={on ? "true" : "false"}
    >
      <ItemRow
        label="Coordinator"
        detail={
          on
            ? "Starts threads and keeps you up to date"
            : "Off. Its chat and threads stay."
        }
        unavailable={saving}
        control={
          <Choice
            label="Coordinator"
            value={on ? "on" : "off"}
            options={[
              { id: "on", label: "On" },
              { id: "off", label: "Off" },
            ]}
            onChange={change}
          />
        }
      />
      {failed ? (
        <p role="alert" className="channel-settings-failure">
          {failed}
        </p>
      ) : null}
    </div>
  )
}

type Page = "general" | "where" | "talk"
const pageNames: Record<Page, string> = {
  general: "General",
  where: "Where it runs",
  talk: "How agents talk",
}

/** The same settings sheet opens from either channel-list layout. */
export function ChannelSettingsButton({
  channelId,
  name,
}: {
  channelId: string
  name: string
}) {
  const [open, setOpen] = useState(false)
  // The sheet opens inside the window's surface, as Settings' do, so its rows
  // read the desktop's type and tones; portalled to the body they have none
  // and fall back to the page's larger text.
  const [home, setHome] = useState<HTMLElement | null>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  return (
    <>
      <IconButton
        ref={trigger}
        data-channel-settings-open={channelId}
        icon="preferences"
        label={`Settings for #${name}`}
        size="sm"
        onClick={(event) => {
          event.stopPropagation()
          setHome(event.currentTarget.closest<HTMLElement>("[data-surface]"))
          setOpen(true)
        }}
      />
      {open && (
        <ChannelSettingsSheet
          channelId={channelId}
          name={name}
          home={home}
          onClose={() => setOpen(false)}
          onClosed={() => trigger.current?.focus()}
        />
      )}
    </>
  )
}

/**
 * A channel's settings: one sheet of a fixed size, a list of pages down its
 * side and the page beside it. The default channel's sheet sets the Nessa
 * level, which every channel follows unless it has its own, and has no
 * coordinator to switch.
 */
function ChannelSettingsSheet({
  channelId,
  name,
  home,
  onClose,
  onClosed,
}: {
  channelId: string
  name: string
  home: HTMLElement | null
  onClose: () => void
  onClosed: () => void
}) {
  const replyStyle = useReplyStyleGateway()
  const scope = gatewayChannelId(channelId)
  const pages: Page[] = [
    ...(scope === undefined ? [] : (["general"] as const)),
    "where",
    ...(replyStyle ? (["talk"] as const) : []),
  ]
  const [page, setPage] = useState<Page>(pages[0] ?? "where")
  const tabs = useRef<(HTMLButtonElement | null)[]>([])
  const panelId = useId()
  // ↑/↓ walk the pages, as a list of tabs down the side should.
  const walk = (event: React.KeyboardEvent, at: number) => {
    const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0
    if (step === 0) return
    event.preventDefault()
    const next = (at + step + pages.length) % pages.length
    setPage(pages[next] as Page)
    tabs.current[next]?.focus()
  }
  return (
    <Dialog
      open
      container={home}
      size="full"
      className="channel-settings settings-surface"
      data-channel-settings=""
      data-channel-settings-page={page}
      onOpenChange={(next) => {
        if (!next) onClose()
      }}
      onCloseAutoFocus={(event) => {
        event.preventDefault()
        onClosed()
      }}
      title={`#${name}`}
      description={
        scope === undefined ? "Defaults for every channel" : "Channel settings"
      }
      aside={
        <div role="tablist" aria-orientation="vertical" className="channel-settings-tabs">
          {pages.map((each, at) => (
            <button
              key={each}
              ref={(node) => {
                tabs.current[at] = node
              }}
              type="button"
              role="tab"
              className="channel-settings-tab"
              aria-selected={each === page}
              aria-controls={panelId}
              tabIndex={each === page ? 0 : -1}
              data-channel-settings-tab={each}
              onClick={() => setPage(each)}
              onKeyDown={(event) => walk(event, at)}
            >
              {pageNames[each]}
            </button>
          ))}
        </div>
      }
    >
      <div
        key={page}
        id={panelId}
        role="tabpanel"
        aria-label={pageNames[page]}
        className="channel-settings-page"
      >
        <h3 className="channel-settings-title">{pageNames[page]}</h3>
        {page === "general" ? (
          <div className="channel-settings-rows">
            <ChannelCoordinatorControl channelId={channelId} />
          </div>
        ) : page === "where" ? (
          <WhereItRuns channelId={channelId} />
        ) : replyStyle ? (
          <div className="channel-settings-rows" data-channel-settings-reply-style="">
            <ReplyStyleControl channelId={scope} gateway={replyStyle} />
          </div>
        ) : null}
      </div>
    </Dialog>
  )
}

const tick = (
  <svg
    className="channel-settings-check"
    viewBox="0 0 14 14"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.6"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
  >
    <path d="M3 7.4 5.8 10 11 4" />
  </svg>
)

type Place = "here" | "cloud"
type Provider = "boat" | "agent37"

/**
 * Where a channel's agents, files and terminals live: this computer, or a
 * cloud box that keeps working while it sleeps. Ticking Cloud box opens the
 * rows a cloud needs (which provider, and its sign-in or key) right there.
 * A channel lives on the gateway it was made on (ADR 1417): the tick shows
 * that gateway, and moving a channel with chats is not offered.
 */
function WhereItRuns({ channelId }: { channelId: string }) {
  const channel = useWorkspaceSelector((state) => selectChannel(state, channelId))
  const gateway = useWorkspaceSelector((state) =>
    selectGateway(state, channel?.gatewayId ?? LOCAL_GATEWAY_ID),
  )
  const onBox = gateway?.cloudBox
  const [place, setPlace] = useState<Place>(onBox ? "cloud" : "here")
  const [provider, setProvider] = useState<Provider>(onBox?.cloud ?? "boat")
  const hereName = gateway && !onBox ? gateway.label : "This Mac"
  const radios = useRef<(HTMLButtonElement | null)[]>([])
  const choose = (next: Place) => {
    setPlace(next)
    radios.current[next === "here" ? 0 : 1]?.focus()
  }
  const keys = (event: React.KeyboardEvent) => {
    if (!["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight"].includes(event.key)) return
    event.preventDefault()
    choose(place === "here" ? "cloud" : "here")
  }
  return (
    <div className="channel-settings-rows" data-channel-where={place}>
      <div role="radiogroup" aria-label="Where it runs" onKeyDown={keys}>
        <button
          ref={(node) => {
            radios.current[0] = node
          }}
          type="button"
          role="radio"
          aria-checked={place === "here"}
          tabIndex={place === "here" ? 0 : -1}
          className="channel-settings-place"
          data-channel-place="here"
          onClick={() => choose("here")}
        >
          <span className="channel-settings-place-text">
            <span>{onBox ? "This Mac" : hereName}</span>
            {!onBox && channel?.folder ? (
              <span className="channel-settings-folder">{channel.folder}</span>
            ) : null}
          </span>
          {tick}
        </button>
        <button
          ref={(node) => {
            radios.current[1] = node
          }}
          type="button"
          role="radio"
          aria-checked={place === "cloud"}
          tabIndex={place === "cloud" ? 0 : -1}
          className="channel-settings-place"
          data-channel-place="cloud"
          onClick={() => choose("cloud")}
        >
          <span className="channel-settings-place-text">
            <span>{onBox ? gateway.label : "Cloud box"}</span>
            {onBox && channel?.folder ? (
              <span className="channel-settings-folder">{channel.folder}</span>
            ) : (
              <span className="channel-settings-place-detail">
                Keeps working while your Mac sleeps
              </span>
            )}
          </span>
          {tick}
        </button>
      </div>
      <div
        className="channel-settings-cloud"
        data-open={place === "cloud" || undefined}
        inert={place !== "cloud"}
      >
        <div>
          <div className="channel-settings-cloud-rows">
            <ItemRow
              label="Provider"
              control={
                <Choice
                  label="Provider"
                  value={provider}
                  options={[
                    { id: "boat", label: "Boat" },
                    { id: "agent37", label: "Agent37" },
                  ]}
                  onChange={setProvider}
                  data-channel-provider=""
                />
              }
            />
            <div key={provider} className="channel-settings-swap">
              {provider === "boat" ? <BoatAccountRow /> : <Agent37KeyRow />}
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

/** Boat's account, in one row: Sign in with Boat, or Sign out. */
function BoatAccountRow() {
  const connection = useBoatConnection()
  const { status, busy } = connection
  const state = status?.state
  const waiting = status?.signIn?.state === "waiting"
  return (
    <ItemRow
      label="Account"
      detail={
        connection.error ||
        (waiting ? "Finish signing in in your browser" : undefined) ||
        (state === undefined && !connection.unread ? "Loading…" : undefined)
      }
      unavailable={!connection.gateway}
      data-channel-boat={state ?? "reading"}
      control={
        state === "connected" ? (
          <RowAction
            disabled={busy}
            onClick={() => void connection.disconnect()}
            data-boat-action="disconnect"
          >
            Sign out
          </RowAction>
        ) : state === "locked" ? (
          <RowAction
            disabled={busy}
            onClick={() => void connection.unlock()}
            data-boat-action="unlock"
          >
            Unlock
          </RowAction>
        ) : waiting ? (
          <span className="channel-settings-actions">
            <RowAction onClick={connection.reopen} data-boat-action="reopen">
              Open Boat
            </RowAction>
            <RowAction
              disabled={busy}
              onClick={() => void connection.cancelSignIn()}
              data-boat-action="cancel-sign-in"
            >
              Cancel
            </RowAction>
          </span>
        ) : status?.signInAvailable === false ? null : (
          <RowAction
            disabled={busy || state === undefined}
            onClick={() => void connection.signIn()}
            data-boat-action="sign-in"
          >
            {busy ? "Opening Boat…" : "Sign in with Boat…"}
          </RowAction>
        )
      }
    />
  )
}

/**
 * Agent37's key, in one row: paste it and Save, or Remove the saved one. Its
 * state comes from the same hook as Settings › Accounts, so both agree.
 */
function Agent37KeyRow() {
  const { gateway, state, unread, busy, error, save, disconnect, unlock, retry } =
    useAgent37Connection()
  const [key, setKey] = useState("")
  const field = useId()
  return (
    <ItemRow
      label={<label htmlFor={state === "connected" ? undefined : field}>API key</label>}
      detail={error || (state === undefined ? "Loading…" : undefined)}
      unavailable={!gateway}
      data-channel-agent37={state ?? "reading"}
      control={
        unread && state === undefined ? (
          <RowAction onClick={retry} data-agent37-action="retry">
            Try again
          </RowAction>
        ) : state === "locked" ? (
          <RowAction disabled={busy} onClick={unlock} data-agent37-action="unlock">
            Unlock
          </RowAction>
        ) : state === "connected" ? (
          <span className="channel-settings-actions">
            <span className="channel-settings-value">Saved</span>
            <RowAction
              disabled={busy}
              onClick={() => void disconnect()}
              data-agent37-action="disconnect"
            >
              Remove
            </RowAction>
          </span>
        ) : (
          <form
            className="channel-settings-actions"
            onSubmit={(event) => {
              event.preventDefault()
              if (key.trim())
                void save(key).then((saved) => {
                  if (saved) setKey("")
                })
            }}
          >
            <input
              id={field}
              className="settings-input channel-settings-key"
              type="password"
              autoComplete="off"
              spellCheck={false}
              placeholder="Paste key"
              value={key}
              disabled={busy || !gateway || state === undefined}
              onChange={(event) => setKey(event.target.value)}
              data-agent37-key=""
            />
            {key.trim() ? (
              <button
                type="submit"
                className="settings-button"
                disabled={busy}
                data-agent37-action="save"
              >
                {busy ? "Saving…" : "Save"}
              </button>
            ) : null}
          </form>
        )
      }
    />
  )
}
