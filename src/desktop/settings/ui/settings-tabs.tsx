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
 * What each tab shows. Theme, icons, tint, and layout are real and
 * remembered; everything else is local prototype state, kept only while
 * Settings is open, so the page has its eventual shape without claiming to
 * change anything.
 */
export const settingsTabPages: Record<SettingsTabId, ComponentType> = {
  general: GeneralTab,
  notifications: NotificationsTab,
  updates: UpdatesTab,
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
        <Choices>
          {desktopThemes.map((option) => (
            <button
              key={option.id}
              type="button"
              role="radio"
              aria-checked={option.id === theme}
              className="settings-choice"
              onClick={() => setTheme(option.id)}
            >
              <span
                className="settings-choice-art settings-theme-preview"
                data-desktop-theme={option.id}
                aria-hidden="true"
              >
                <ChoiceCheck />
              </span>
              <span>{option.label}</span>
            </button>
          ))}
        </Choices>
      </SettingGroup>
      <SettingGroup id="icon-family" footnote="Used for the window's own controls.">
        <Choices>
          {iconFamilies.map((option) => (
            <button
              key={option.id}
              type="button"
              role="radio"
              aria-checked={option.id === family}
              className="settings-choice"
              onClick={() => setFamily(option.id)}
            >
              {/* Each preview draws through its own family, whatever the window uses. */}
              <DesktopIconProvider icons={iconFamilyDrawings[option.id]}>
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
              <span>{option.label}</span>
            </button>
          ))}
        </Choices>
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
  const [greeting, setGreeting] = usePrototype(true)
  return (
    <Group footnote="Choose or frame the picture from Customize on the home header.">
      <Row id="tint-from-picture">
        <Toggle checked={tint} onChange={setTint} />
      </Row>
      <Row id="show-greeting">
        <Toggle checked={greeting} onChange={setGreeting} />
      </Row>
    </Group>
  )
}

function MotionTab() {
  const [motion, setMotion] = usePrototype<"system" | "full" | "reduced">("system")
  const [drift, setDrift] = usePrototype(true)
  return (
    <Group footnote="System follows Reduce motion in macOS Accessibility settings.">
      <Row id="animations">
        <Segmented
          value={motion}
          onChange={setMotion}
          options={[
            { id: "system", label: "System" },
            { id: "full", label: "Full" },
            { id: "reduced", label: "Reduced" },
          ]}
        />
      </Row>
      <Row id="drifting-light">
        <Toggle checked={drift} onChange={setDrift} />
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
  const [list, setList] = usePrototype(true)
  const [cmdClick, setCmdClick] = usePrototype(true)
  return (
    <>
      <SettingGroup id="workspace-layout">
        <Choices>
          {workspaceLayouts.map((option) => (
            <button
              key={option.id}
              type="button"
              role="radio"
              aria-checked={option.id === layout}
              className="settings-choice"
              onClick={() => setLayout(option.id)}
            >
              <LayoutSketch id={option.id} />
              <span>{option.label}</span>
            </button>
          ))}
        </Choices>
      </SettingGroup>
      <Group>
        <Row id="show-session-list">
          <Toggle checked={list} onChange={setList} />
        </Row>
        <Row id="cmd-click-beside">
          <Toggle checked={cmdClick} onChange={setCmdClick} />
        </Row>
      </Group>
    </>
  )
}

function SessionsTab() {
  const [keep, setKeep] = usePrototype<"week" | "month" | "always">("month")
  const [runningFirst, setRunningFirst] = usePrototype(true)
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
        <Toggle checked={runningFirst} onChange={setRunningFirst} />
      </Row>
    </Group>
  )
}

const shortcutGroups = [
  {
    title: "Window",
    keys: [
      ["Settings", "⌘ ,"],
      ["Toggle sidebar", "⌘ B"],
      ["Toggle session list", "⌥ ⌘ S"],
      ["Search", "⌘ K"],
    ],
  },
  {
    title: "Sessions",
    keys: [
      ["New session", "⌘ N"],
      ["Split right", "⌘ \\"],
      ["Split down", "⇧ ⌘ \\"],
      ["Close pane", "⌘ W"],
      ["Focus pane 1–4", "⌘ 1–4"],
    ],
  },
] as const

function KeyboardTab() {
  return (
    <>
      {shortcutGroups.map((group) => (
        <Group key={group.title} title={group.title}>
          {group.keys.map(([label, keys]) => (
            <ItemRow
              key={label}
              label={label}
              control={<kbd className="settings-kbd">{keys}</kbd>}
            />
          ))}
        </Group>
      ))}
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
    <Group footnote="Models come from the agents on this Mac; a provider without one needs its own key.">
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
    </Group>
  )
}

/* ——— Connections ——— */

function AgentsTab() {
  const agents = [
    { id: "claude", name: "Claude Code", detail: "Installed · 2.4.1" },
    { id: "codex", name: "Codex", detail: "Installed · 0.61.0" },
    { id: "opencode", name: "OpenCode", detail: "Not signed in" },
  ] as const
  return (
    <Group footnote="Nessa runs the agents already on this Mac; it doesn't install its own copies.">
      {agents.map((agent) => (
        <ItemRow
          key={agent.id}
          label={agent.name}
          detail={agent.detail}
          leading={
            <span className="settings-tile">
              <AgentMark id={agent.id} name={agent.name} />
            </span>
          }
        />
      ))}
    </Group>
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
