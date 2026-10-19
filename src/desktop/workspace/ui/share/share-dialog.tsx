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
 * to the host bound the schema publishes and lets the gateway refuse the
 * rest; after every change it shows the list as the gateway reads it again,
 * never a guess from the outcome. Change and Revoke name the revision they
 * were built on, so a grant changed in another window is refused rather than
 * overwritten.
 *
 * Mounted once by the shell; it draws nothing while no dialog is open.
 */
import { shallowEqual } from "react-redux"
import { Button } from "@nessa-ui/react/button"
import { CheckboxList } from "@nessa-ui/react/checkbox-list"
import { Choices } from "@nessa-ui/react/choices"
import { Dialog } from "@nessa-ui/react/dialog"
import { Input } from "@nessa-ui/react/input"
import { LoadingText } from "@nessa-ui/react/loading-text"
import {
  SegmentedControl,
  SegmentedControlOption,
} from "@nessa-ui/react/segmented-control"
import { SettingsGroup, SettingsRow } from "@nessa-ui/react/settings-group"
import type { ShareDialog as ShareDialogState } from "../../application/workspace-state"
import {
  askRevoke,
  cancelRevoke,
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
  selectShareWaitingOn,
} from "../../adapters/store/selectors"
import {
  draftFromGrant,
  emptyDraft,
  hostFits,
  requestFrom,
  type ShareDraft,
  type ShareGrant,
  type ShareRole,
  type SharingChoices,
  type ToolPreset,
} from "../../model/sharing"
import {
  changeFailureCopy,
  choicesFailureCopy,
  deviceDetail,
  doneCopy,
  draftReasonCopy,
  driveNote,
  grantSummary,
  presetCopy,
  readFailureCopy,
  roleCopy,
  waitingCopy,
} from "./share-copy"
import { openSettings } from "../../../settings"
import "./share-dialog.css"

const roles: readonly ShareRole[] = ["read", "comment", "drive"]
const presets: readonly (ToolPreset | "custom")[] = [
  "read_only_tools",
  "no_shell",
  "ask_everything",
  "all",
  "custom",
]

/** The dialog, while one is open; one per opening, so nothing of an earlier opening's is drawn in it. */
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
  // The one rule for whether the list and the form may be acted on
  // (`formHeld`): a change or a read on its way, or a list that predates
  // this window's last change. Everything below derives from it.
  const held = useWorkspaceSelector(selectShareHeld)
  const waiting = useWorkspaceSelector(selectShareWaitingOn)
  // Whether a change may begin is the use case's rule (`canChange`), not this view's.
  const changeable = useWorkspaceSelector(selectShareCanChange)
  const { draft, grants, choices } = dialog
  const outcome = requestFrom(draft, grants ?? [], choices?.limits ?? noRoom)
  const onDraft = (next: ShareDraft) => void dispatch(draftShare({ draft: next }))
  const holds = grants?.find((grant) => grant.credentialId === draft.credentialId)
  // Why the draft cannot be sent yet, once the form has been read.
  const draftReason =
    grants && choices && !held && !outcome.ok && outcome.reason !== "no-target"
      ? outcome.reason
      : null
  // Share is offered only with a device to share with; the × closes the dialog.
  const submittable = (choices?.targets.length ?? 0) > 0
  // Choosing a device starts from what it holds, or from nothing: never
  // from what another device's grant left in the form.
  const choose = (credentialId: string) => {
    const grant = grants?.find((each) => each.credentialId === credentialId)
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
      data-share-dialog=""
      data-share-state={
        grants === null ? (dialog.reading ? "reading" : "unread") : held ? "held" : "read"
      }
      footer={
        waiting || draftReason || submittable ? (
          <>
            {/* Beside the button it holds back, so it is read where it matters. */}
            {waiting ? (
              <p className="share-reason nessa-text-2" data-share-reason={waiting}>
                {waitingCopy[waiting]}
              </p>
            ) : draftReason && choices ? (
              <p className="share-reason nessa-text-2" data-share-reason={draftReason}>
                {draftReasonCopy(draftReason, choices.limits)}
                {draftReason === "changed" ? (
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
            {submittable ? (
              <Button
                data-share-action="submit"
                disabled={!outcome.ok || !changeable}
                onClick={() => {
                  // Sends the draft as the store holds it, not this render's copy.
                  void dispatch(shareSession())
                }}
              >
                {holds ? "Update" : "Share"}
              </Button>
            ) : null}
          </>
        ) : null
      }
    >
      <div className="share-dialog">
        {dialog.readFailure ? (
          <div
            className="share-alert nessa-text-2"
            role="alert"
            data-share-read-failure={dialog.readFailure.part}
          >
            <p>
              {dialog.readFailure.part === "grants"
                ? readFailureCopy[dialog.readFailure.reason]
                : choicesFailureCopy[dialog.readFailure.reason]}
            </p>
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
        <p
          className="share-notice nessa-text-2"
          role="status"
          data-share-notice={dialog.settled?.outcome ?? ""}
        >
          {dialog.settled
            ? dialog.settled.outcome === "done"
              ? doneCopy[dialog.settled.change.kind]
              : changeFailureCopy[dialog.settled.outcome]
            : ""}
        </p>
        {grants ? (
          <>
            <Grants
              grants={grants}
              choices={choices}
              busy={!changeable}
              onChange={choose}
              confirming={dialog.confirming?.credentialId ?? null}
              onAsk={(credentialId) => void dispatch(askRevoke({ credentialId }))}
              onCancel={() => void dispatch(cancelRevoke())}
              onRevoke={() => void dispatch(revokeShare())}
            />
            {choices ? (
              <fieldset className="share-fieldset" disabled={held} data-share-form="">
                <Form
                  choices={choices}
                  draft={draft}
                  onDraft={onDraft}
                  onChoose={choose}
                />
              </fieldset>
            ) : null}
          </>
        ) : dialog.readFailure ? null : (
          <LoadingText className="share-quiet nessa-text-2" data-share-loading="" />
        )}
      </div>
    </Dialog>
  )
}

/** The bounds before anything has been read: no room for anything. */
const noRoom = { patterns: 0, hosts: 0, hostBytes: 0 }

function Grants({
  grants,
  choices,
  busy,
  onChange,
  confirming,
  onAsk,
  onCancel,
  onRevoke,
}: {
  grants: readonly ShareGrant[]
  /** What the form offers, when it could be read: without it, a grant can still be revoked. */
  choices: SharingChoices | null
  busy: boolean
  onChange: (credentialId: string) => void
  /** The device whose Revoke is being asked (workspace state, `confirming`). */
  confirming: string | null
  onAsk: (credentialId: string) => void
  onCancel: () => void
  /** Revoke what was asked about, naming its revision as it was then. */
  onRevoke: () => void
}) {
  // Nothing shared says itself: no "Shared with" group.
  if (grants.length === 0) return null
  return (
    <SettingsGroup title="Shared with" data-share-grants="">
      {grants.map((grant) => {
        const target = choices?.targets.find(
          (each) => each.credentialId === grant.credentialId,
        )
        return (
          <SettingsRow
            key={grant.credentialId}
            label={
              choices === null
                ? "Device"
                : target
                  ? "Linked device"
                  : "Not a linked device"
            }
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
                    onClick={onCancel}
                  >
                    Cancel
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    className="share-revoke"
                    data-share-action="confirm-revoke"
                    disabled={busy}
                    onClick={onRevoke}
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
                    onClick={() => onAsk(grant.credentialId)}
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

/**
 * Nothing to share with yet: one line, and the way to Settings › Linked
 * devices, where a device is linked. Closing the dialog first, as Settings
 * covers the window.
 */
function NoDevices() {
  const dispatch = useWorkspaceDispatch()
  return (
    <p className="share-empty nessa-text-2" data-share-targets="none">
      <span>No linked devices yet.</span>
      <Button
        variant="link"
        size="sm"
        className="share-link"
        data-share-action="link-device"
        onClick={() => {
          dispatch(closeShare())
          openSettings("devices")
        }}
      >
        Link a Device
      </Button>
    </p>
  )
}

function Form({
  choices,
  draft,
  onDraft,
  onChoose,
}: {
  choices: SharingChoices
  draft: ShareDraft
  onDraft: (draft: ShareDraft) => void
  onChoose: (credentialId: string) => void
}) {
  if (choices.targets.length === 0) return <NoDevices />
  return (
    <section className="share-form" aria-label="Share with a device">
      <h3 className="share-heading nessa-text-2">Share with</h3>
      <Choices
        aria-label="Device"
        data-share-targets=""
        options={choices.targets.map((target) => ({
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
        <DriveFields choices={choices} draft={draft} onDraft={onDraft} />
      ) : null}
    </section>
  )
}

function DriveFields({
  choices,
  draft,
  onDraft,
}: {
  choices: SharingChoices
  draft: ShareDraft
  onDraft: (draft: ShareDraft) => void
}) {
  const custom = draft.preset === "custom"
  const room = choices.limits.hosts
  // The hosts on offer, then any the grant holds that are not on offer now:
  // shown, so they can be kept or unchecked, never dropped unseen.
  const away = draft.hosts.filter((host) => !choices.commandHosts.includes(host))
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
          ...choices.commandHosts.map((host) => {
            const fits = hostFits(choices.limits, host)
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
        count={({ selected, max }) => `${selected} of ${max} selected`}
        limitReason="No more room for hosts"
        emptyMessage="The gateway runs your commands on no host, so this grant can’t run any."
      />
      <p className="share-quiet nessa-text-2" data-share-notes="">
        {driveNote}
      </p>
    </div>
  )
}
