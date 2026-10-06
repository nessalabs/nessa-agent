/** Sample sessions in the Starred section: #desktop-app and #release. */
import {
  edited,
  exchange,
  models,
  ran,
  read,
  sampleApprovalOptions,
  searched,
  text,
  type SampleSession,
} from "./sample-content"

export const starredSamples: readonly SampleSession[] = [
  {
    id: "split-panes",
    channelId: "desktop-app",
    title: "Split panes for the chat workspace",
    model: models.opus,
    status: "running",
    updated: 0,
    started: 22,
    preview: "Laying out panes as fractions so a third pane never squeezes the first.",
    now: "Capping the workspace at four panes, then wiring the drop target",
    pinned: true,
    activity: ["Editing workspace-panes.tsx", 38],
    messages: [
      [
        "user",
        22,
        [
          text(
            "Let the workspace hold two or three conversations side by side. Dragging a session onto the chat area should open it in a split.",
          ),
        ],
      ],
      [
        "agent",
        18,
        [
          read("src/desktop/ui/desktop-app.tsx"),
          read("split-view/split-view-math.ts"),
          searched("SplitView · 6 results"),
          text(
            "The shell already owns one `SplitView` for the sidebars, so I'd keep panes in a second, nested group. Each pane stores a **fraction** of the chat area rather than a pixel width, which means:",
          ),
          {
            kind: "list",
            items: [
              "Resizing the window keeps every pane's proportions.",
              "A new pane takes its share from the one it was dropped beside.",
              "The focused pane is lit by `--desktop-light-edge`; the others step back slightly.",
            ],
          },
        ],
      ],
      [
        "user",
        6,
        [text("Good. Cap it at four — past that nothing is readable at 1440.")],
      ],
      [
        "agent",
        2,
        [
          edited("src/desktop/ui/workspace-panes.tsx", 142, 8),
          text(
            "Capped at four. Dropping another replaces the pane under the pointer instead, and the drop target says so before you let go.",
          ),
        ],
      ],
    ],
  },
  {
    id: "signing",
    channelId: "desktop-app",
    title: "Release build signing fails on CI",
    model: models.astra,
    status: "needs-you",
    updated: 4,
    started: 60,
    preview: "Wants to run the notarisation step with the release keychain.",
    now: "Waiting to import the new certificate before signing again",
    unread: true,
    approval: {
      id: "signing-import",
      command: "security import build/nessa-dev.p12 -k release.keychain",
      reason: "Imports a signing certificate into the release keychain.",
      origin: { kind: "agent" },
      options: sampleApprovalOptions,
    },
    messages: exchange(
      "The macOS job fails at codesign since yesterday. Find out why.",
      "The keychain the job unlocks expired its certificate on the 25th. The new one is already in the vault; the job just needs to import it before signing.",
      5,
      [ran("gh run view 1184 --log-failed"), read(".github/workflows/release.yml")],
    ),
  },
  {
    id: "frost",
    channelId: "desktop-app",
    title: "Frosted sidebar loses blur mid-slide",
    model: models.sonnet,
    updated: 60,
    preview:
      "Opacity below 1 made the peek its own backdrop root. Fixed by sliding instead.",
    pinned: true,
    messages: exchange(
      "When the sidebar peeks in, the blur switches off for a few frames. Why?",
      "An ancestor with opacity below 1 becomes the backdrop root for the glass inside it, so while the peek faded in, the blur had nothing behind it to sample. It now slides fully opaque, like the Dock, and only turns invisible once it is off-screen.",
      60,
      [read("src/desktop/styles.css")],
    ),
  },
  {
    id: "traffic-lights",
    channelId: "desktop-app",
    title: "Titlebar traffic-light inset",
    model: models.sonnet,
    updated: 130,
    preview:
      "Centres the native buttons on the same 24px row as the titlebar's controls.",
    messages: exchange(
      "The traffic lights sit a few pixels higher than the sidebar toggle.",
      "The window's `trafficLightPosition` now centres them 24px below the top, on the same row as the toggle, measured through the accessibility frames of the close button.",
      130,
      [read("src-tauri/tauri.conf.json")],
    ),
  },
  {
    id: "page-mode",
    channelId: "desktop-app",
    title: "Composer page mode threshold",
    model: models.minimax,
    updated: 26 * 60,
    preview: "Seven lines in, three lines out, so the page never flickers at the edge.",
    messages: exchange(
      "The composer flips to page mode and back while typing around six lines.",
      "It used one threshold both ways. I gave it hysteresis: it becomes a page at seven lines and only returns to a card below three.",
      26 * 60,
    ),
  },
  {
    id: "tint",
    channelId: "desktop-app",
    title: "Tint the window from a picture",
    model: models.sol,
    updated: 2 * 24 * 60 + 60,
    preview: "Five colours in OKLab, and the three theme lights derived from them.",
    messages: exchange(
      "Could the header picture lend the window its colours?",
      "It clusters the picture's pixels into five main colours in OKLab and derives the three theme lights from them, held within the built-in themes' ranges.",
      2 * 24 * 60 + 60,
      [read("src/desktop/model/image-palette.ts")],
    ),
  },
  {
    id: "night-scene-4k",
    channelId: "desktop-app",
    title: "Night scene on 4K displays",
    model: models.opus,
    updated: 6 * 24 * 60,
    preview:
      "Caps the scene at 290px so large windows get a finer band, not a coarser one.",
    messages: exchange(
      "The rain looks chunky on the Studio Display.",
      "The scene scaled with the window's height. It now stops growing at 290px and crops sideways instead, so a big window shows more of the street at the same fineness.",
      6 * 24 * 60,
    ),
  },
  {
    id: "release-notes",
    channelId: "release",
    title: "Draft 0.9 release notes",
    model: models.sonnet,
    status: "running",
    updated: 2,
    started: 9,
    preview: "Grouping 41 merged PRs into what someone upgrading would notice first.",
    now: "Grouping 41 merged PRs by what someone upgrading notices first",
    unread: true,
    activity: ["Reading merged PRs", 95],
    messages: exchange(
      "Write release notes for 0.9 from everything merged since 0.8.3.",
      "Reading the merged PRs now. I'm grouping them by what someone upgrading would notice — the desktop window first, then the gateway, then fixes.",
      3,
      [ran("git log v0.8.3..main --merges")],
    ),
  },
  {
    id: "notarize",
    channelId: "release",
    title: "Notarize the macOS build",
    model: models.sol,
    status: "needs-you",
    updated: 6,
    started: 14,
    preview: "Wants to upload the signed build to Apple for notarization.",
    now: "Waiting to upload the signed 0.9.0 build to Apple",
    approval: {
      id: "notarize-submit",
      command:
        "xcrun notarytool submit Nessa_0.9.0_aarch64.dmg --keychain-profile nessa --wait",
      reason:
        "Uploads the signed build to Apple for notarization. This uses the release keychain profile.",
      origin: { kind: "agent" },
      options: sampleApprovalOptions,
    },
    messages: [["user", 14, [text("Notarize the 0.9.0 build once it is signed.")]]],
  },
  {
    id: "tauri-bump",
    channelId: "release",
    title: "Bump Tauri to 2.4",
    model: models.sol,
    updated: 180,
    preview: "Upgraded; the window-state plugin changed its config key.",
    messages: exchange(
      "Bump Tauri to 2.4 and fix whatever breaks.",
      "Done. The only break was the window-state plugin renaming its config key, which I updated in tauri.conf.json.",
      180,
    ),
  },
]
