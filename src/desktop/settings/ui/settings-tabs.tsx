import { BoatRow } from "./boat-row"
import { VaultPassphraseControl } from "../../../vault"
import { useState, type ComponentType } from "react"
import { hasNativeHost, replayOnboarding } from "../../../host"
import { useIconFamilyPreference } from "../../adapters/icon-family-preference"
import { useTintFromPicture } from "../../adapters/header-image"
import { useThemePreference } from "../../adapters/theme-preference"
import { iconFamilies } from "../../model/icon-family"
import { chordLabel } from "../../model/keyboard"
import { isMac } from "../../adapters/platform"
import type { SettingsTabId } from "../model/settings-catalogue"
import { version as appVersion } from "../../../../package.json"
import { desktopThemes } from "../../model/theme"
import { workspaceLayouts, type WorkspaceLayoutId } from "../../model/workspace-layout"
import { useWorkspaceLayoutPreference } from "../../adapters/workspace-layout-preference"
import { useMotionPreference } from "../../adapters/motion-preference"
import {
  useBesidePreference,
  useDriftPreference,
  useGreetingPreference,
  useSideRailPreference,
  usePictureInConversationsPreference,
  useRunningFirstPreference,
  useSubagentsPreview,
} from "../../adapters/window-preferences"
import { motionChoices } from "../../model/motion"
import {
  selectSessionListChosen,
  shortcutNames,
  toggleSessionList,
  workspaceShortcuts,
  useWorkspaceDispatch,
  useWorkspaceSelector,
} from "../../workspace"
import {
  DesktopIcon,
  DesktopIconProvider,
  iconFamilyDrawings,
  type DesktopIconRole,
} from "../../ui/icons"
import {
  Choices,
  Group,
  ItemRow,
  Row,
  RowAction,
  Segmented,
  SettingGroup,
  Toggle,
} from "./settings-controls"
import { GithubRow } from "./github-row"
import { VoiceKeyRow } from "./voice-key-row"
import { SavedLoginsGroup } from "./saved-logins"
import { IntegrationsTab } from "./integrations-tab"
import { LinkedDevicesTab } from "./linked-devices-tab"
import { NativeHooksRows } from "./native-hooks-rows"
import { PermissionRuleRows, TrustedAppRows } from "./permission-rule-rows"
import { ReviewNoticeRows } from "./review-notice-rows"

/**
 * What each tab shows. Every setting shown is real and remembered: theme,
 * icons, tint, the picture in conversations, greeting, motion, drifting
 * light, layout, the subagents preview, the session list, ⌘-click, running
 * first. A setting not built yet is not in the catalogue at all, so nothing
 * here looks as if it works when it does nothing.
 * Integrations is the gateway's stored MCP servers where the window has a
 * gateway, and pending where it has none (`integrations-tab.tsx`).
 * Linked devices pairs and revokes where the window has a gateway, and is
 * pending where it has none (`linked-devices-tab.tsx`).
 * Native hooks lists Claude and Codex native hooks where the window has a gateway,
 * and rests those rows where it has none (`native-hooks-rows.tsx`).
 */
export const settingsTabPages: Record<SettingsTabId, ComponentType> = {
  theme: ThemeTab,
  header: HeaderTab,
  motion: MotionTab,
  layout: LayoutTab,
  sessions: SessionsTab,
  keyboard: KeyboardTab,
  accounts: AccountsTab,
  integrations: IntegrationsTab,
  devices: LinkedDevicesTab,
  access: AccessTab,
  "native-hooks": NativeHooksRows,
  experimental: ExperimentalTab,
  setup: SetupTab,
  about: AboutTab,
}

/* ——— Appearance ——— */

/** What the Icons choice shows of each family: a few of the window's everyday icons. */
const iconPreview: readonly DesktopIconRole[] = [
  "sidebar",
  "search",
  "newSession",
  "thinking",
  "send",
]

function ThemeTab() {
  const [theme, setTheme] = useThemePreference()
  const [family, setFamily] = useIconFamilyPreference()
  return (
    <>
      <SettingGroup id="theme-colour">
        <Choices
          options={desktopThemes}
          value={theme}
          onChange={setTheme}
          art={(id) => (
            <span
              className="settings-choice-art settings-theme-preview"
              data-desktop-theme={id}
              aria-hidden="true"
            >
              <ChoiceCheck />
            </span>
          )}
        />
      </SettingGroup>
      <SettingGroup id="icon-family" note="Used for the window's own controls.">
        <Choices
          options={iconFamilies}
          value={family}
          onChange={setFamily}
          art={(id) => (
            // Each preview draws through its own family, whatever the window uses.
            <DesktopIconProvider icons={iconFamilyDrawings[id]}>
              <span
                className="settings-choice-art settings-icon-preview"
                aria-hidden="true"
              >
                {iconPreview.map((role) => (
                  <DesktopIcon key={role} name={role} />
                ))}
                <ChoiceCheck />
              </span>
            </DesktopIconProvider>
          )}
        />
      </SettingGroup>
    </>
  )
}

/** The small mark on a chosen card. Drawn by Nessa's family whatever the card previews. */
function ChoiceCheck() {
  return (
    <span className="settings-choice-check">
      <DesktopIconProvider icons={iconFamilyDrawings.nessa}>
        <DesktopIcon name="check" />
      </DesktopIconProvider>
    </span>
  )
}

function HeaderTab() {
  const [tint, setTint] = useTintFromPicture()
  const [greeting, setGreeting] = useGreetingPreference()
  const [inConversations, setInConversations] = usePictureInConversationsPreference()
  return (
    <Group note="Choose or frame the picture from Customize on the home header.">
      <Row id="tint-from-picture">
        <Toggle checked={tint} onChange={setTint} />
      </Row>
      <Row id="picture-in-conversations">
        <Toggle
          checked={inConversations === "on"}
          onChange={(on) => setInConversations(on ? "on" : "off")}
        />
      </Row>
      <Row id="show-greeting">
        <Toggle
          checked={greeting === "on"}
          onChange={(on) => setGreeting(on ? "on" : "off")}
        />
      </Row>
    </Group>
  )
}

function MotionTab() {
  const [motion, setMotion] = useMotionPreference()
  const [drift, setDrift] = useDriftPreference()
  return (
    <Group note="System follows Reduce motion in macOS Accessibility settings.">
      <Row id="animations">
        <Segmented value={motion} onChange={setMotion} options={motionChoices} />
      </Row>
      <Row id="drifting-light">
        <Toggle checked={drift === "on"} onChange={(on) => setDrift(on ? "on" : "off")} />
      </Row>
    </Group>
  )
}

/* ——— Workspace ——— */

/** A small drawing of each layout, so the choice is seen rather than read. */
function LayoutSketch({ id }: { id: WorkspaceLayoutId }) {
  // Three columns: sidebar, session list, chat. Sessions in sidebar: a wider
  // sidebar holding the sessions, then chat.
  const columns = id === "columns" ? ["side", "list", "main"] : ["tree", "main"]
  return (
    <span className="settings-choice-art settings-sketch" aria-hidden="true">
      {columns.map((kind, index) => (
        <span key={index} data-kind={kind} />
      ))}
      <ChoiceCheck />
    </span>
  )
}

function LayoutTab() {
  const [layout, setLayout] = useWorkspaceLayoutPreference()
  const dispatch = useWorkspaceDispatch()
  // The same choice as the layout's own toggle (⌥⌘S): the workspace holds it.
  const list = useWorkspaceSelector(selectSessionListChosen)
  const [beside, setBeside] = useBesidePreference()
  return (
    <>
      <SettingGroup id="workspace-layout">
        <Choices
          options={workspaceLayouts}
          value={layout}
          onChange={setLayout}
          art={(id) => <LayoutSketch id={id} />}
        />
      </SettingGroup>
      <Group>
        <Row id="show-session-list">
          <Toggle
            checked={list}
            onChange={(open) => dispatch(toggleSessionList({ open }))}
          />
        </Row>
        <Row id="cmd-click-beside">
          <Toggle
            checked={beside === "on"}
            onChange={(on) => setBeside(on ? "on" : "off")}
          />
        </Row>
      </Group>
    </>
  )
}

function SessionsTab() {
  const [runningFirst, setRunningFirst] = useRunningFirstPreference()
  return (
    <Group>
      <Row id="running-first">
        <Toggle
          checked={runningFirst === "on"}
          onChange={(on) => setRunningFirst(on ? "on" : "off")}
        />
      </Row>
    </Group>
  )
}

/**
 * The window's keyboard, read from the one map the keys themselves run
 * (`workspaceShortcuts`), so what Settings lists is what the keys do.
 */
function KeyboardTab() {
  return (
    <>
      <Group title="Window">
        <ItemRow
          label="Settings"
          control={
            <kbd className="settings-kbd">
              {chordLabel({ code: "Comma", command: true }, isMac)}
            </kbd>
          }
        />
      </Group>
      <Group title="Panes and sessions">
        {workspaceShortcuts.map((binding) => (
          <ItemRow
            key={binding.command}
            label={shortcutNames[binding.command]}
            control={
              <kbd className="settings-kbd">{chordLabel(binding.chord, isMac)}</kbd>
            }
          />
        ))}
      </Group>
    </>
  )
}

/* ——— Connections ——— */

function AccountsTab() {
  return (
    <>
      <Group>
        <GithubRow />
        <VoiceKeyRow />
        <BoatRow />
        <Row id="vault-passphrase">
          <VaultPassphraseControl />
        </Row>
      </Group>
      <SavedLoginsGroup />
    </>
  )
}

/* ——— Privacy & Permissions ——— */

function AccessTab() {
  return (
    <>
      <PermissionRuleRows />
      <TrustedAppRows />
      <ReviewNoticeRows />
    </>
  )
}

/* ——— Advanced ——— */

/** Previews of features not settled yet, each a switch. */
function ExperimentalTab() {
  const [preview, setPreview] = useSubagentsPreview()
  const [rail, setRail] = useSideRailPreference()
  return (
    <Group>
      <Row id="subagents-preview">
        <Toggle
          checked={preview === "on"}
          onChange={(on) => setPreview(on ? "on" : "off")}
        />
      </Row>
      <Row id="side-rail">
        <Toggle checked={rail === "on"} onChange={(on) => setRail(on ? "on" : "off")} />
      </Row>
    </Group>
  )
}

/**
 * First-run setup, to see again. Only the desktop app has a setup window to
 * open, so elsewhere the page says that instead of offering a button that
 * could do nothing.
 */
function SetupTab() {
  const [state, setState] = useState<"idle" | "opening" | "failed">("idle")
  if (!hasNativeHost())
    return (
      <div className="settings-empty">
        <DesktopIcon name="advanced" />
        <p>Setup can be replayed in the Nessa app.</p>
      </div>
    )
  const replay = () => {
    setState("opening")
    replayOnboarding().then(
      () => setState("idle"),
      () => setState("failed"),
    )
  }
  return (
    <Group
      note={state === "failed" ? "Setup did not open. Try again in a moment." : undefined}
    >
      <Row id="replay-onboarding">
        <RowAction disabled={state === "opening"} onClick={replay}>
          Replay
        </RowAction>
      </Row>
    </Group>
  )
}

/* ——— About ——— */

function AboutTab() {
  return (
    <>
      <p className="settings-about">
        <strong>nessa</strong> <span>Studio</span>
      </p>
      <Group>
        <Row id="version">
          <span className="settings-value">{appVersion}</span>
        </Row>
      </Group>
    </>
  )
}
