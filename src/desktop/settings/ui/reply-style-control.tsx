import { Button } from "@nessa-ui/react/button"
import {
  createContext,
  useContext,
  useEffect,
  useId,
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

function words(choice: ReplyStyleChoice): string {
  return choice.mode === "plain"
    ? "Nessa’s plain style"
    : choice.mode === "off"
      ? "No style: the agent’s own"
      : "Your own words"
}

/**
 * How agents talk, for one level: the Nessa level (no `channelId`) or one
 * channel's own. Each role is one choice; a channel's "Same as Nessa"
 * follows the Nessa level. Plain and off save as they are chosen; your own
 * words save when you press Save.
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
  const save = async (role: ReplyStyleRole, choice: ReplyStyleChoice | undefined) => {
    if (!gateway || busy) return
    const current = ++revision.current
    setBusy(true)
    setError("")
    try {
      const value = await gateway.set(role, choice, channelId)
      if (current === revision.current) setStyles(value)
    } catch {
      if (current === revision.current) {
        setError("Couldn’t confirm the change.")
        setStyles(undefined)
      }
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
          onSave={(choice) => void save(role, choice)}
        />
      ))}
      <p className="settings-boat-note">
        New chats start with this. A chat already started keeps its style.
      </p>
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
  onSave: (choice: ReplyStyleChoice | undefined) => void
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
  const [selection, setSelection] = useState<Selection>()
  const [text, setText] = useState("")
  const field = useId()
  const savedText = own?.mode === "custom" ? (own.text ?? "") : ""
  useEffect(() => {
    setSelection(saved)
    setText(savedText)
  }, [saved, savedText])
  const options: { id: Selection; label: string }[] = [
    ...(channel ? [{ id: "default" as const, label: "Same as Nessa" }] : []),
    { id: "plain", label: "Plain" },
    { id: "off", label: "Off" },
    { id: "custom", label: "Your own" },
  ]
  const trimmed = text.trim()
  const tooLong = new TextEncoder().encode(trimmed).length > MAX_TEXT_BYTES
  const detail = loading
    ? "Loading…"
    : styles
      ? selection === "default"
        ? `Same as Nessa: ${words(styles.effective[role])}`
        : selection === "custom"
          ? undefined
          : words(styles.effective[role])
      : undefined
  return (
    <ItemRow
      label={label}
      detail={detail}
      unavailable={unavailable}
      data-reply-style-role={role}
      control={
        <Choice
          label={`How the ${label.toLowerCase()} talk${role === "coordinator" ? "s" : ""}`}
          value={selection}
          options={options}
          onChange={(next) => {
            setSelection(next)
            if (next === "default") onSave(undefined)
            else if (next !== "custom") onSave({ mode: next })
          }}
        />
      }
    >
      {selection === "custom" ? (
        <div className="settings-field">
          <label htmlFor={field} className="settings-field-label">
            Tell {role === "coordinator" ? "the coordinator" : "threads"} how to write
          </label>
          <textarea
            id={field}
            className="settings-input settings-input-wrapped"
            data-reply-style-text={role}
            rows={4}
            value={text}
            placeholder="For example: Answer in one short paragraph. Use plain words."
            readOnly={unavailable}
            onChange={(event) => setText(event.target.value)}
          />
          {tooLong ? (
            <span role="status" className="settings-boat-note">
              This can be up to 4 KB.
            </span>
          ) : null}
          <div>
            <Button
              className="settings-button"
              type="button"
              data-reply-style-save={role}
              disabled={
                unavailable ||
                trimmed.length === 0 ||
                tooLong ||
                (own?.mode === "custom" && own.text === trimmed)
              }
              onClick={() => onSave({ mode: "custom", text: trimmed })}
            >
              Save
            </Button>
          </div>
        </div>
      ) : null}
    </ItemRow>
  )
}
