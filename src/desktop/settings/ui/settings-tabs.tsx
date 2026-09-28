import type { ComponentType } from "react"
import { AgentMark } from "../../../onboarding/ui/agent-mark"
import { useIconFamilyPreference } from "../../adapters/icon-family-preference"
import { useTintFromPicture } from "../../adapters/header-image"
import { useThemePreference } from "../../adapters/theme-preference"
import { agentForProvider, composerProviders } from "../../model/composer-options"
import { iconFamilies } from "../../model/icon-family"
import type { SettingsTabId } from "../model/settings-catalogue"
import { desktopThemes } from "../../model/theme"
import { workspaceLayouts, type WorkspaceLayoutId } from "../../model/workspace-layout"
import { useWorkspaceLayoutPreference } from "../../adapters/workspace-layout-preference"
import { useMotionPreference } from "../../adapters/motion-preference"
import {
  useBesidePreference,
  useDriftPreference,
  useGreetingPreference,
  useRunningFirstPreference,
} from "../../adapters/window-preferences"
import { motionChoices } from "../../model/motion"
import { useAgentsOverviewPreference } from "../../experiments/agents-overview"
import {
  chordLabel,
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
  PendingAction,
  Row,
  Segmented,
  SettingGroup,
  Toggle,
  usePrototype,
} from "./settings-controls"

/**
 * What each tab shows. Every setting the window owns is real and
 * remembered: theme, icons, tint, greeting, motion, drifting light, layout,
 * the session list, ⌘-click, running first. What lives outside the window —
 * the host, the gateway, an account — is marked not available yet in the
 * catalogue (`pending`), and its row says so with its control disabled.
 */
export const settingsTabPages: Record<SettingsTabId, ComponentType> = {
  general: GeneralTab,
  notifications: NotificationsTab,
  updates: UpdatesTab,
  experimental: ExperimentalTab,
  theme: ThemeTab,
  header: HeaderTab,
  motion: MotionTab,
  layout: LayoutTab,
  sessions: SessionsTab,
  keyboard: KeyboardTab,
  defaults: DefaultsTab,
  providers: ProvidersTab,
  agents: AgentsTab,
  accounts: AccountsTab,
  integrations: IntegrationsTab,
  access: AccessTab,
  data: DataTab,
  about: AboutTab,
}

/* ——— General ——— */

function GeneralTab() {
  const [launch, setLaunch] = usePrototype(true)
  const [menuBar, setMenuBar] = usePrototype(true)
  const [open, setOpen] = usePrototype<"home" | "last">("last")
  return (
    <>
      <Group>
        <Row id="open-at-login">
          <Toggle checked={launch} onChange={setLaunch} />
        </Row>
        <Row id="menu-bar">
          <Toggle checked={menuBar} onChange={setMenuBar} />
        </Row>
      </Group>
      <Group title="When the window opens">
        <Row id="window-opens-to">
          <Segmented
            value={open}
            onChange={setOpen}
            options={[
              { id: "home", label: "Home" },
              { id: "last", label: "Last session" },
            ]}
          />
        </Row>
      </Group>
    </>
  )
}

function NotificationsTab() {
  const [needsYou, setNeedsYou] = usePrototype(true)
  const [finished, setFinished] = usePrototype(false)
  const [sound, setSound] = usePrototype(false)
  return (
    <Group
      title="Let me know"
      footnote="Only while the window is in the background or closed."
    >
      <Row id="notify-needs-you">
        <Toggle checked={needsYou} onChange={setNeedsYou} />
      </Row>
      <Row id="notify-finished">
        <Toggle checked={finished} onChange={setFinished} />
      </Row>
      <Row id="notify-sound">
        <Toggle checked={sound} onChange={setSound} />
      </Row>
    </Group>
  )
}

function UpdatesTab() {
  const [automatic, setAutomatic] = usePrototype(true)
  const [channel, setChannel] = usePrototype<"stable" | "beta">("stable")
  return (
    <>
      <Group>
        <ItemRow
          label="Nessa is up to date"
          detail="Version 0.1.0 (prototype)"
          control={<PendingAction>Check now</PendingAction>}
        />
      </Group>
      <Group>
        <Row id="update-automatically">
          <Toggle checked={automatic} onChange={setAutomatic} />
        </Row>
        <Row id="update-channel">
          <Segmented
            value={channel}
            onChange={setChannel}
            options={[
              { id: "stable", label: "Stable" },
              { id: "beta", label: "Beta" },
            ]}
          />
        </Row>
      </Group>
    </>
  )
}

/** Previews, each off until turned on (`src/desktop/experiments/`). */
function ExperimentalTab() {
  const [overview, setOverview] = useAgentsOverviewPreference()
  return (
    <Group footnote="Experiments may change or go away in a later version.">
      <Row id="agents-overview">
        <Toggle
          checked={overview === "on"}
          onChange={(on) => setOverview(on ? "on" : "off")}
        />
      </Row>
    </Group>
  )
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
      <SettingGroup id="theme-light">
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
      <SettingGroup id="icon-family" footnote="Used for the window's own controls.">
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
  return (
    <Group footnote="Choose or frame the picture from Customize on the home header.">
      <Row id="tint-from-picture">
        <Toggle checked={tint} onChange={setTint} />
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
    <Group footnote="System follows Reduce motion in macOS Accessibility settings.">
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
  // sidebar holding the sessions, then chat. Classic: sidebar, home, panel.
  const columns =
    id === "columns"
      ? ["side", "list", "main"]
      : id === "sidebar"
        ? ["tree", "main"]
        : ["side", "main", "panel"]
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
  const [keep, setKeep] = usePrototype<"week" | "month" | "always">("month")
  const [runningFirst, setRunningFirst] = useRunningFirstPreference()
  return (
    <Group footnote="Pinned sessions are always kept.">
      <Row id="keep-sessions">
        <Segmented
          value={keep}
          onChange={setKeep}
          options={[
            { id: "week", label: "A week" },
            { id: "month", label: "A month" },
            { id: "always", label: "Always" },
          ]}
        />
      </Row>
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
              {chordLabel({ code: "Comma", command: true })}
            </kbd>
          }
        />
      </Group>
      <Group title="Workspace">
        {workspaceShortcuts.map((binding) => (
          <ItemRow
            key={binding.command}
            label={shortcutNames[binding.command]}
            control={<kbd className="settings-kbd">{chordLabel(binding.chord)}</kbd>}
          />
        ))}
      </Group>
    </>
  )
}

/* ——— Models ——— */

function DefaultsTab() {
  const [model, setModel] = usePrototype<"opus" | "sonnet" | "gpt">("opus")
  const [thinking, setThinking] = usePrototype<"low" | "medium" | "high" | "max">(
    "medium",
  )
  const [fast, setFast] = usePrototype(false)
  return (
    <Group title="New sessions start with">
      <Row id="default-model">
        <Segmented
          value={model}
          onChange={setModel}
          options={[
            { id: "opus", label: "Claude Opus 5" },
            { id: "sonnet", label: "Sonnet 5" },
            { id: "gpt", label: "GPT-6 Astra" },
          ]}
        />
      </Row>
      <Row id="default-thinking">
        <Segmented
          value={thinking}
          onChange={setThinking}
          options={[
            { id: "low", label: "Low" },
            { id: "medium", label: "Medium" },
            { id: "high", label: "High" },
            { id: "max", label: "Max" },
          ]}
        />
      </Row>
      <Row id="fast-mode">
        <Toggle checked={fast} onChange={setFast} />
      </Row>
    </Group>
  )
}

function ProvidersTab() {
  return (
    <SettingGroup
      id="providers"
      footnote="Models come from the agents on this Mac; a provider without one needs its own key."
    >
      {composerProviders.map((provider) => {
        const agent = agentForProvider(provider.id)
        const count = provider.models.length
        return (
          <ItemRow
            key={provider.id}
            label={provider.label}
            detail={`${count} ${count === 1 ? "model" : "models"}${agent ? "" : " · needs an API key"}`}
            leading={
              <span className="settings-tile">
                {agent ? (
                  <AgentMark id={agent} name={provider.label} />
                ) : (
                  provider.label.charAt(0)
                )}
              </span>
            }
            control={agent ? null : <PendingAction>Add key…</PendingAction>}
          />
        )
      })}
    </SettingGroup>
  )
}

/* ——— Connections ——— */

function AgentsTab() {
  // Which agents are installed, and at what version, the host will say; until
  // then each is named, and none is claimed.
  const agents = [
    { id: "claude", name: "Claude Code" },
    { id: "codex", name: "Codex" },
    { id: "opencode", name: "OpenCode" },
  ] as const
  return (
    <SettingGroup
      id="agents"
      footnote="Nessa runs the agents already on this Mac; it doesn't install its own copies."
    >
      {agents.map((agent) => (
        <ItemRow
          key={agent.id}
          label={agent.name}
          leading={
            <span className="settings-tile">
              <AgentMark id={agent.id} name={agent.name} />
            </span>
          }
        />
      ))}
    </SettingGroup>
  )
}

function AccountsTab() {
  return (
    <Group>
      <Row id="nessa-account">
        <PendingAction>Sign in…</PendingAction>
      </Row>
      <Row id="github">
        <PendingAction>Connect…</PendingAction>
      </Row>
    </Group>
  )
}

function IntegrationsTab() {
  return (
    <SettingGroup
      id="mcp-servers"
      footnote="Servers added here are offered to every agent that supports MCP."
    >
      <div className="settings-empty">
        <DesktopIcon name="connections" />
        <p>No servers yet</p>
        <PendingAction>Add server…</PendingAction>
      </div>
    </SettingGroup>
  )
}

/* ——— Privacy & Permissions ——— */

function AccessTab() {
  const [access, setAccess] = usePrototype<"ask" | "edits" | "full">("ask")
  const [remember, setRemember] = usePrototype(true)
  return (
    <Group
      title="By default, agents may"
      footnote="Each session can change this from the shield in its composer."
    >
      <Row id="default-access">
        <Segmented
          value={access}
          onChange={setAccess}
          options={[
            { id: "ask", label: "Ask first" },
            { id: "edits", label: "Edit files" },
            { id: "full", label: "Full access" },
          ]}
        />
      </Row>
      <Row id="remember-approvals">
        <Toggle checked={remember} onChange={setRemember} />
      </Row>
    </Group>
  )
}

function DataTab() {
  const [crashes, setCrashes] = usePrototype(false)
  return (
    <>
      <Group>
        <Row id="crash-reports">
          <Toggle checked={crashes} onChange={setCrashes} />
        </Row>
      </Group>
      <Group>
        <Row id="session-history">
          <PendingAction>Show in Finder</PendingAction>
        </Row>
      </Group>
    </>
  )
}

/* ——— About ——— */

function AboutTab() {
  return (
    <>
      <div className="settings-about">
        <span className="desktop-mark settings-about-mark" aria-hidden="true" />
        <div>
          <strong>nessa</strong> <span>Studio</span>
        </div>
      </div>
      <Group>
        <Row id="version">
          <span className="settings-value">0.1.0 (prototype)</span>
        </Row>
      </Group>
    </>
  )
}
