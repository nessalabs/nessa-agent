/**
 * The owner's share dialog (nessa_ui#128, #129): who a session is shared
 * with, a Revoke for each, and the form that grants a linked device a role —
 * for Drive, its tool policy and the hosts it may run commands on. The kit
 * draws every control (`Dialog`, `Choices`, `SegmentedControl`,
 * `CheckboxList`, `SettingsGroup`, `Input`, `Button`); this wires them to the
 * workspace's share state (`usecases/sharing.ts`) and says each outcome in
 * words (`share-copy.ts`).
 *
 * The rules are the gateway's. The dialog holds the hosts a person may check
 * to the room the published bounds leave (`hostRoom`) and lets the gateway
 * refuse the rest; after every change it shows the list as the gateway reads
 * it again, never a guess from the outcome.
 *
 * Mounted once by the shell; it draws nothing while no dialog is open.
 */
import { useState } from "react"
import { shallowEqual } from "react-redux"
import { Button } from "@nessa-ui/react/button"
import { CheckboxList } from "@nessa-ui/react/checkbox-list"
import { Choices } from "@nessa-ui/react/choices"
import { Dialog, DialogClose } from "@nessa-ui/react/dialog"
import { Input } from "@nessa-ui/react/input"
import {
  SegmentedControl,
  SegmentedControlOption,
} from "@nessa-ui/react/segmented-control"
import { SettingsGroup, SettingsRow } from "@nessa-ui/react/settings-group"
import type { ShareDialog as ShareDialogState } from "../../application/workspace-state"
import {
  closeShare,
  draftShare,
  readShares,
  restartShare,
  revokeShare,
  shareSession,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectSession,
  selectShareCanChange,
  selectShareDialog,
  selectShareHeld,
  selectShareHeldElsewhere,
} from "../../adapters/store/selectors"
import {
  draftFromGrant,
  emptyDraft,
  hostFits,
  hostRoom,
  draftPatternCount,
  requestFrom,
  type ShareDraft,
  type ShareRole,
  type SharingRead,
  type ToolPreset,
} from "../../model/sharing"
import {
  changeFailureCopy,
  deviceDetail,
  doneCopy,
  draftReasonCopy,
  driveNotes,
  grantSummary,
  heldElsewhereCopy,
  presetCopy,
  readFailureCopy,
  roleCopy,
} from "./share-copy"
import "./share-dialog.css"

const roles: readonly ShareRole[] = ["read", "comment", "drive"]
const presets: readonly (ToolPreset | "custom")[] = [
  "read_only_tools",
  "no_shell",
  "ask_everything",
  "all",
  "custom",
]

/** The dialog, while one is open; one per opening, so its local asks (a Revoke being confirmed) go with it. */
export function ShareDialogHost() {
  const dialog = useWorkspaceSelector(selectShareDialog)
  if (!dialog) return null
  return <ShareDialog key={dialog.opening} dialog={dialog} />
}

function ShareDialog({ dialog }: { dialog: ShareDialogState }) {
  const dispatch = useWorkspaceDispatch()
  const title = useWorkspaceSelector(
    (state) => selectSession(state, dialog.sessionId)?.title,
    shallowEqual,
  )
  // Held while a change, or the read that starts the draft again after it,
  // is on its way (`formHeld`): an edit then would be lost to that read.
  const held = useWorkspaceSelector(selectShareHeld)
  const elsewhere = useWorkspaceSelector(selectShareHeldElsewhere)
  // Whether a change may begin is the use case's rule (`canChange`), not this view's.
  const changeable = useWorkspaceSelector(selectShareCanChange)
  const { draft } = dialog
  const read = dialog.value
  const outcome = requestFrom(draft, read ?? { grants: [], limits: noRoom })
  const onDraft = (next: ShareDraft) => void dispatch(draftShare({ draft: next }))
  const holds = read?.grants.find((grant) => grant.credentialId === draft.credentialId)
  // Choosing a device starts from what it holds, or from nothing: never
  // from what another device's grant left in the form.
  const choose = (credentialId: string) => {
    const grant = read?.grants.find((each) => each.credentialId === credentialId)
    onDraft(grant ? draftFromGrant(grant) : emptyDraft(credentialId))
  }
  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) dispatch(closeShare())
      }}
      size="lg"
      title={title ? `Share “${title}”` : "Share"}
      description="Linked devices you share this conversation with can follow it from there."
      data-share-dialog=""
      data-share-state={
        held ? "held" : dialog.reading ? "reading" : read ? "read" : "unread"
      }
      footer={
        <>
          {/* Beside the button it holds back, so it is read where it matters. */}
          {elsewhere ? (
            <p className="share-reason nessa-text-2" data-share-reason="elsewhere">
              {heldElsewhereCopy}
            </p>
          ) : read && !outcome.ok && outcome.reason !== "no-target" ? (
            <p className="share-reason nessa-text-2" data-share-reason={outcome.reason}>
              {draftReasonCopy(outcome.reason, read.limits)}
              {outcome.reason === "changed" ? (
                <>
                  {" "}
                  <Button
                    variant="outline"
                    size="sm"
                    data-share-action="restart"
                    disabled={held}
                    onClick={() => void dispatch(restartShare())}
                  >
                    Start Again
                  </Button>
                </>
              ) : null}
            </p>
          ) : null}
          <DialogClose asChild>
            <Button variant="outline" data-share-action="close">
              Done
            </Button>
          </DialogClose>
          <Button
            data-share-action="submit"
            disabled={!outcome.ok || !changeable}
            onClick={() => {
              if (outcome.ok) void dispatch(shareSession({ request: outcome.request }))
            }}
          >
            {holds ? "Update" : "Share"}
          </Button>
        </>
      }
    >
      <div className="share-dialog">
        {dialog.readFailure ? (
          <div
            className="share-alert nessa-text-2"
            role="alert"
            data-share-read-failure=""
          >
            <p>{readFailureCopy[dialog.readFailure]}</p>
            <Button
              variant="outline"
              size="sm"
              data-share-action="retry"
              disabled={dialog.reading !== null}
              onClick={() => void dispatch(readShares())}
            >
              Try Again
            </Button>
          </div>
        ) : null}
        <p className="share-notice nessa-text-2" role="status" data-share-notice="">
          {dialog.notice
            ? dialog.notice.outcome === "done"
              ? doneCopy[dialog.notice.change.kind]
              : changeFailureCopy[dialog.notice.outcome]
            : ""}
        </p>
        {read ? (
          <>
            <Grants
              read={read}
              busy={!changeable}
              onChange={choose}
              onRevoke={(credentialId) => void dispatch(revokeShare({ credentialId }))}
            />
            <fieldset className="share-fieldset" disabled={held} data-share-form="">
              <Form read={read} draft={draft} onDraft={onDraft} onChoose={choose} />
            </fieldset>
          </>
        ) : dialog.readFailure ? null : (
          <p className="share-quiet nessa-text-2" data-share-loading="">
            Reading who this is shared with…
          </p>
        )}
      </div>
    </Dialog>
  )
}

/** The bounds before anything has been read: no room for anything. */
const noRoom = { patterns: 0, hosts: 0, hostBytes: 0, entries: 0 }

function Grants({
  read,
  busy,
  onChange,
  onRevoke,
}: {
  read: SharingRead
  busy: boolean
  onChange: (credentialId: string) => void
  onRevoke: (credentialId: string) => void
}) {
  const [confirming, setConfirming] = useState<string | null>(null)
  if (read.grants.length === 0)
    return (
      <p className="share-quiet nessa-text-2" data-share-grants="none">
        Not shared with any device yet.
      </p>
    )
  return (
    <SettingsGroup title="Shared with" data-share-grants="">
      {read.grants.map((grant) => {
        const target = read.targets.find(
          (each) => each.credentialId === grant.credentialId,
        )
        return (
          <SettingsRow
            key={grant.credentialId}
            label={target ? "Linked device" : "Not a linked device"}
            detail={
              confirming === grant.credentialId
                ? "Revoke this device’s access to the conversation?"
                : `${
                    target
                      ? deviceDetail(target.credentialId, target.issuedAt)
                      : `ID …${grant.credentialId.slice(-6)}`
                  } · ${grantSummary(grant)}`
            }
            data-share-grant={grant.credentialId}
            control={
              confirming === grant.credentialId ? (
                // Asked first, as Settings asks: a Drive grant's policy is
                // not kept anywhere once it is revoked.
                <span className="share-actions">
                  <Button
                    variant="ghost"
                    size="sm"
                    data-share-action="cancel-revoke"
                    onClick={() => setConfirming(null)}
                  >
                    Cancel
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    className="share-revoke"
                    data-share-action="confirm-revoke"
                    disabled={busy}
                    onClick={() => {
                      setConfirming(null)
                      onRevoke(grant.credentialId)
                    }}
                  >
                    Revoke
                  </Button>
                </span>
              ) : (
                <span className="share-actions">
                  {target ? (
                    <Button
                      variant="ghost"
                      size="sm"
                      data-share-action="change"
                      disabled={busy}
                      onClick={() => onChange(grant.credentialId)}
                    >
                      Change
                    </Button>
                  ) : null}
                  <Button
                    variant="outline"
                    size="sm"
                    className="share-revoke"
                    data-share-action="revoke"
                    disabled={busy}
                    onClick={() => setConfirming(grant.credentialId)}
                  >
                    Revoke
                  </Button>
                </span>
              )
            }
          />
        )
      })}
    </SettingsGroup>
  )
}

function Form({
  read,
  draft,
  onDraft,
  onChoose,
}: {
  read: SharingRead
  draft: ShareDraft
  onDraft: (draft: ShareDraft) => void
  onChoose: (credentialId: string) => void
}) {
  if (read.targets.length === 0)
    return (
      <p className="share-quiet nessa-text-2" data-share-targets="none">
        No linked device can be shared with. Link one in Settings › Linked devices.
      </p>
    )
  return (
    <section className="share-form" aria-label="Share with a device">
      <h3 className="share-heading nessa-text-2">Share with</h3>
      <Choices
        aria-label="Device"
        data-share-targets=""
        options={read.targets.map((target) => ({
          value: target.credentialId,
          label: "Linked device",
          description: deviceDetail(target.credentialId, target.issuedAt),
        }))}
        value={draft.credentialId ?? ""}
        onValueChange={onChoose}
      />
      <h3 className="share-heading nessa-text-2">Role</h3>
      <SegmentedControl
        aria-label="Role"
        data-share-role=""
        value={draft.role}
        onValueChange={(value) => {
          const role = roles.find((each) => each === value)
          if (role) onDraft({ ...draft, role })
        }}
      >
        {roles.map((role) => (
          <SegmentedControlOption key={role} value={role}>
            {roleCopy[role].label}
          </SegmentedControlOption>
        ))}
      </SegmentedControl>
      <p className="share-quiet nessa-text-2">{roleCopy[draft.role].description}</p>
      {draft.role === "drive" ? (
        <DriveFields read={read} draft={draft} onDraft={onDraft} />
      ) : null}
    </section>
  )
}

function DriveFields({
  read,
  draft,
  onDraft,
}: {
  read: SharingRead
  draft: ShareDraft
  onDraft: (draft: ShareDraft) => void
}) {
  const custom = draft.preset === "custom"
  const room = hostRoom(read.limits, draftPatternCount(draft))
  // The hosts on offer, then any the grant holds that are not on offer now:
  // shown, so they can be kept or unchecked, never dropped unseen.
  const away = draft.hosts.filter((host) => !read.commandHosts.includes(host))
  return (
    <div className="share-drive" data-share-drive="">
      <h3 className="share-heading nessa-text-2">Tools</h3>
      <Choices
        aria-label="Tools"
        data-share-presets=""
        options={presets.map((preset) => ({
          value: preset,
          label: presetCopy[preset].label,
          description: presetCopy[preset].description,
        }))}
        value={draft.preset}
        onValueChange={(value) => {
          const preset = presets.find((each) => each === value)
          if (preset) onDraft({ ...draft, preset })
        }}
      />
      {custom ? (
        <div className="share-patterns">
          <label className="share-field nessa-text-2">
            <span>Allow</span>
            <Input
              data-share-pattern="allow"
              value={draft.allow}
              placeholder="Read, Grep, run"
              onChange={(event) => onDraft({ ...draft, allow: event.target.value })}
            />
          </label>
          <label className="share-field nessa-text-2">
            <span>Deny</span>
            <Input
              data-share-pattern="deny"
              value={draft.deny}
              placeholder="Write, Edit"
              onChange={(event) => onDraft({ ...draft, deny: event.target.value })}
            />
          </label>
          <p className="share-quiet nessa-text-2">
            Tool names, separated by commas. * matches anything, and a deny wins over an
            allow.
          </p>
        </div>
      ) : null}
      <CheckboxList
        label="Hosts it may run commands on"
        data-share-hosts=""
        options={[
          ...read.commandHosts.map((host) => {
            const fits = hostFits(read.limits, host)
            return {
              value: host,
              label: host,
              disabled: !fits,
              disabledReason: fits ? undefined : "Name too long for a grant",
            }
          }),
          ...away.map((host) => ({
            value: host,
            label: host,
            description: "Not taking your commands now",
          })),
        ]}
        value={draft.hosts}
        onValueChange={(next) => onDraft({ ...draft, hosts: next })}
        max={room}
        // Past the room the patterns leave — a pattern typed after the hosts
        // were checked — the hint says so rather than "3 of 2".
        count={({ selected, max }) =>
          max !== undefined && selected > max
            ? `${selected} selected, room for ${max}`
            : `${selected} of ${max} selected`
        }
        limitReason={
          room < read.limits.hosts
            ? "No more room: hosts and tool patterns share one limit"
            : "No more room for hosts"
        }
        emptyMessage="The gateway runs your commands on no host, so this grant can’t run any."
      />
      <ul className="share-notes nessa-text-2" data-share-notes="">
        <li>{driveNotes.hosts}</li>
        <li>{driveNotes.unasked}</li>
        <li>{driveNotes.enforceable}</li>
      </ul>
    </div>
  )
}
