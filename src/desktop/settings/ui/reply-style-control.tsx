import { Button } from "@nessa-ui/react/button"
import { Dialog } from "@nessa-ui/react/dialog"
import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react"
import type { ReplyStyleChoice, ReplyStyleRole, ReplyStyles } from "@nessa/client"
import type { ReplyStyleGateway } from "../adapters/reply-style-gateway"
import { Choice, ItemRow } from "./settings-controls"

const ReplyStyleContext = createContext<ReplyStyleGateway | undefined>(undefined)
export function ReplyStyleProvider({
  gateway,
  children,
}: {
  gateway?: ReplyStyleGateway
  children: ReactNode
}) {
  return (
    <ReplyStyleContext.Provider value={gateway}>{children}</ReplyStyleContext.Provider>
  )
}
export function useReplyStyleGateway() {
  return useContext(ReplyStyleContext)
}

/** The most bytes of a person's own style, as the gateway takes it. */
const MAX_TEXT_BYTES = 4096
type Selection = "default" | "plain" | "off" | "custom"
const roles: { role: ReplyStyleRole; label: string }[] = [
  { role: "coordinator", label: "Coordinator" },
  { role: "thread", label: "Threads" },
]

/**
 * How agents talk, for one level: the Nessa level (no `channelId`) or one
 * channel's own. Each role is one choice; a channel's "Same as Nessa"
 * follows the Nessa level. Plain and off save as they are chosen; "Your
 * own" opens a place to write, and saves only when Save there is pressed.
 */
export function ReplyStyleControl({
  channelId,
  gateway,
}: {
  channelId?: string
  gateway?: ReplyStyleGateway
}) {
  const revision = useRef(0)
  const [styles, setStyles] = useState<ReplyStyles>()
  const [error, setError] = useState("")
  const [busy, setBusy] = useState(false)
  const [retry, setRetry] = useState(0)
  useEffect(() => {
    const current = ++revision.current
    setStyles(undefined)
    setError("")
    setBusy(false)
    void gateway
      ?.read(channelId)
      .then((value) => {
        if (current === revision.current) setStyles(value)
      })
      .catch(() => {
        if (current === revision.current) setError("Couldn’t read this.")
      })
    return () => {
      revision.current += 1
    }
  }, [gateway, channelId, retry])
  const save = async (
    role: ReplyStyleRole,
    choice: ReplyStyleChoice | undefined,
  ): Promise<boolean> => {
    if (!gateway || busy) return false
    const current = ++revision.current
    setBusy(true)
    setError("")
    try {
      const value = await gateway.set(role, choice, channelId)
      if (current === revision.current) setStyles(value)
      return true
    } catch {
      if (current === revision.current) {
        setError("Couldn’t confirm the change.")
        setStyles(undefined)
      }
      return false
    } finally {
      if (current === revision.current) setBusy(false)
    }
  }
  return (
    <div
      role="group"
      aria-label="How agents talk"
      data-reply-style={channelId ?? "nessa"}
      className="settings-reply-style"
    >
      {roles.map(({ role, label }) => (
        <RoleRow
          key={role}
          role={role}
          label={label}
          channel={channelId !== undefined}
          styles={styles}
          unavailable={!gateway || !styles || busy}
          loading={!!gateway && !styles && !error}
          onSave={(choice) => save(role, choice)}
        />
      ))}
      {error && (
        <p role="alert" className="settings-boat-note">
          {error}{" "}
          <Button
            className="settings-button"
            type="button"
            onClick={() => setRetry((value) => value + 1)}
          >
            Reload
          </Button>
        </p>
      )}
    </div>
  )
}

function RoleRow({
  role,
  label,
  channel,
  styles,
  unavailable,
  loading,
  onSave,
}: {
  role: ReplyStyleRole
  label: string
  channel: boolean
  styles: ReplyStyles | undefined
  unavailable: boolean
  loading: boolean
  onSave: (choice: ReplyStyleChoice | undefined) => Promise<boolean>
}) {
  const own = styles && (channel ? styles.channel?.[role] : styles.nessa[role])
  // The Nessa level with no choice is Nessa's plain style; a channel with
  // none follows the Nessa level.
  const saved: Selection | undefined = !styles
    ? undefined
    : own
      ? own.mode
      : channel
        ? "default"
        : "plain"
  const savedText = own?.mode === "custom" ? (own.text ?? "") : ""
  // "Your own" opens a place to write, on its own; the row keeps showing
  // what is saved until Save there says otherwise.
  const [writing, setWriting] = useState(false)
  const row = useRef<HTMLDivElement>(null)
  const options: { id: Selection; label: string }[] = [
    ...(channel ? [{ id: "default" as const, label: "Same as Nessa" }] : []),
    { id: "plain", label: "Plain" },
    { id: "off", label: "Off" },
    { id: "custom", label: "Your own" },
  ]
  // The choice says it all; your own words open in the writer, from View.
  const detail = loading ? "Loading…" : undefined
  return (
    <div ref={row} className="settings-reply-style-role">
      <ItemRow
        label={label}
        detail={detail}
        unavailable={unavailable}
        data-reply-style-role={role}
        data-reply-style-saved={saved}
        control={
          <span className="settings-reply-style-controls">
            {saved === "custom" ? (
              <Button
                className="settings-reply-style-view"
                variant="ghost"
                size="sm"
                type="button"
                data-reply-style-view={role}
                disabled={unavailable}
                onClick={() => setWriting(true)}
              >
                View
              </Button>
            ) : null}
            <Choice
              label={`How the ${label.toLowerCase()} talk${role === "coordinator" ? "s" : ""}`}
              value={saved}
              options={options}
              onChange={(next) => {
                if (next === "custom") setWriting(true)
                else if (next === "default") void onSave(undefined)
                else if (next !== saved) void onSave({ mode: next })
              }}
            />
          </span>
        }
      />
      {writing ? (
        <StyleWriter
          role={role}
          saved={savedText}
          home={row.current?.closest<HTMLElement>("[data-surface]") ?? null}
          // Opened from the menu, its item is gone by close: focus goes back
          // to the row's menu, the control that chose Your own.
          returnFocus={() =>
            row.current?.querySelector<HTMLElement>("[data-choice-trigger]")?.focus()
          }
          onClose={() => setWriting(false)}
          onSave={(text) => onSave({ mode: "custom", text })}
        />
      ) : null}
    </div>
  )
}

/**
 * A place to write the person's own style, and nothing else: a dialog over
 * the settings with one field, Cancel and Save. Nothing is saved until Save,
 * and Cancel leaves the row as it was.
 */
function StyleWriter({
  role,
  saved,
  home,
  returnFocus,
  onClose,
  onSave,
}: {
  role: ReplyStyleRole
  saved: string
  home: HTMLElement | null
  returnFocus: () => void
  onClose: () => void
  onSave: (text: string) => Promise<boolean>
}) {
  const [text, setText] = useState(saved)
  const [saving, setSaving] = useState(false)
  const [failed, setFailed] = useState(false)
  const field = useRef<HTMLTextAreaElement>(null)
  const trimmed = text.trim()
  const tooLong = new TextEncoder().encode(trimmed).length > MAX_TEXT_BYTES
  const who = role === "coordinator" ? "the coordinator" : "threads"
  const save = () => {
    if (saving || trimmed.length === 0 || tooLong) return
    setSaving(true)
    setFailed(false)
    void onSave(trimmed).then((done) => {
      setSaving(false)
      if (done) onClose()
      else setFailed(true)
    })
  }
  return (
    <Dialog
      open
      container={home}
      size="lg"
      className="settings-style-writer"
      data-reply-style-writer={role}
      title={`How ${who} write${role === "coordinator" ? "s" : ""}`}
      description="New chats are told this, in your words. A chat already started keeps its style."
      initialFocus={field}
      onReturnFocus={returnFocus}
      onOpenChange={(open) => {
        if (!open && !saving) onClose()
      }}
      footer={
        <>
          <Button variant="ghost" type="button" disabled={saving} onClick={onClose}>
            Cancel
          </Button>
          <Button
            type="button"
            data-reply-style-save={role}
            disabled={saving || trimmed.length === 0 || tooLong || trimmed === saved}
            onClick={save}
          >
            {saving ? "Saving…" : "Save"}
          </Button>
        </>
      }
    >
      <textarea
        ref={field}
        aria-label={`How ${who} write${role === "coordinator" ? "s" : ""}`}
        className="settings-input settings-input-wrapped settings-style-writer-field"
        data-reply-style-text={role}
        rows={10}
        value={text}
        placeholder="For example: Answer in one short paragraph. Use plain words."
        readOnly={saving}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault()
            save()
          }
        }}
      />
      {tooLong ? (
        <p role="status" className="settings-boat-note">
          This can be up to 4 KB.
        </p>
      ) : failed ? (
        <p role="alert" className="settings-boat-note">
          Couldn’t save that. Try again in a moment.
        </p>
      ) : null}
    </Dialog>
  )
}
