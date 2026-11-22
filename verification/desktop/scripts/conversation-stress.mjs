#!/usr/bin/env node
import { Run } from "./lib/stress-run.mjs"
import { reportStressStep, withStepFailureEvidence } from "./lib/stress-step-evidence.mjs"
import {
  boatProbe,
  channelPlacementSettings,
  channelRuntimeSettings,
  openAccounts,
  pickChoice,
} from "./lib/boat-runtime-settings.mjs"
import {
  changeVaultPassphrase,
  unlockRequestedVault,
  unlockVault,
  VAULT_PASSPHRASE,
} from "./lib/vault.mjs"
/**
 * The conversation stress check: a person using one conversation for many
 * rounds through the real gateway and the desktop window, with the scripted
 * agent standing in for the model. Seeded, so a failing run is replayed by
 * its numbers.
 *
 * Every message on the main conversation is typed into the window's composer
 * and sent with its Send button, never through the client library, because
 * the client always makes fresh request ids and so hid the desktop's own
 * send bugs. Between rounds the plan (`lib/stress-plan.mjs`) mixes in:
 *
 *   reply       a plain turn; the window shows its echo
 *   permission  the agent asks; a card is answered in the window — or, under
 *               full access, nothing is asked and the tool runs
 *   reviews     one turn asks `--reviews` reviews in a row, each a card
 *   large       one review larger than the old 16 KB display limit
 *   stop        Send turns into Stop while the turn runs; Stop cancels it.
 *               While it runs, the window shows work one way, shimmering
 *               words, with no turning arc in a pane's header, the sidebar
 *               or the line under the turn: the chat's channel name
 *               shimmers, its settings show only under the pointer;
 *               stopped, the name is plain again
 *   parent-stop Stop cancels the parent, owned child and grandchild without a report restart
 *   fail        the turn fails; the next message is sent in the same conversation
 *   cloud       Agent37 is picked as the default cloud in Settings ›
 *               Accounts (offered once Boat and Agent37 are both connected): a thread's VM is made on Agent37 (the
 *               fixture's fake one) and stays there after Boat is picked
 *               again, while the next thread's goes to Boat; a thread in a
 *               channel placed on Agent37 goes to Agent37 while Boat is
 *               chosen
 *   restore     a restored conversation is opened and shows its saved record;
 *               another, whose source file is moved aside, refuses Send and
 *               keeps the draft
 *   usageLimit  the turn ends in a usage limit; the card shows, the project
 *               preference is turned on and back off, and the turn is not resumed
 *   signIn      the agent refuses the turn for sign-in; the card offers the
 *               sign-in for where the run was (Authenticate for a Boat VM,
 *               this computer's otherwise), and the next message retires it
 *   switch      the access shield picks another mode, sometimes mid-turn,
 *               where the gateway takes it at once
 *   raise       the shield raises the main chat from Ask to Full while its
 *               turn waits on the first of three reviews: the gateway runs it
 *               at Full while the turn still runs, the open review is allowed
 *               by the raise with no click, and the turn asks nothing more
 *   restart     the gateway is killed (crash) or stopped (shutdown) mid-turn
 *               and started again; the turn is resumed and the window follows
 *   delegate    the agent creates a thread and messages it; the thread's
 *               report comes back as a turn of its own
 *   await       the agent messages a thread and waits for it with one
 *               subscribe call naming the sent turn: the thread's reply comes
 *               back in that call, and no report follows. The thread names
 *               where it runs; made from a channel's chat (the `channel`
 *               step), it is still listed in that channel
 *   webhook     one granted conversation advances to a fixed source Head after
 *               a browser turn; independent native rows prove zero attempts
 *               under the unchanged loopback destination refusal policy
 *   peer        a paired peer gateway, granted comment and drive, leaves a
 *               note and sends a turn; the window shows the note as a grant
 *               and the turn, and the gateway's view and grant record name
 *               that peer
 *   aside       another conversation is opened in the window and talked to
 *   watch       the agent reads a new thread with more reads at once than
 *               one conversation may hold: some wait, the rest are refused,
 *               and a read after them is taken again
 *   window      the sidebar is hidden with ⌘B, another conversation is
 *               opened beside the main one with ⌘-click and asks the agent
 *               what the window shows: it is the focused pane on the right,
 *               the main conversation on the left, the sidebar closed; the
 *               focused chat then opens two panel widgets (the talk board and
 *               a pin from another server), the front tab is the other one,
 *               ⌥⌘B hides and shows the panel, and focusing the main chat
 *               reads that chat's empty panel rather than the one beside;
 *               the pane beside is closed with ⌘W and the sidebar shown
 *               again, and the main conversation, asked in its composer, is
 *               the whole window;
 *               beside, the panes are regions of one flat chat surface that
 *               reaches the window's top, right and bottom edges: no pane
 *               draws a fill or rim of its own, and a hairline down the
 *               middle of the gutter between them divides them; no tab
 *               strip is drawn for the chats, each pane has one header in
 *               the titlebar's row, clear of the window's controls, and its
 *               picture runs up to the window's top edge — with the session
 *               list shown and with the panes alone; with the list hidden,
 *               choosing the channel in the sidebar shows it again
 *   long        another conversation takes `--long` turns of long streamed
 *               answers through the gateway; the last answer is whole, and
 *               still whole after its agent is closed and its history opened
 *               again, no slower for its length than `reopenGrowthMs`
 *               allows; the message sent in the window after them is answered
 *   fresh       a conversation is started in the window, from a new
 *               session's home, while `FRESH_BURST` others are started
 *               through the gateway at the same moment; its first message is
 *               sent and answered, and nothing the window or they sent is
 *               refused
 *   park        another conversation is left idle, unwatched, until the
 *               gateway parks its agent (its lease ends `idle`); it is opened
 *               in the window and the message sent there starts the agent
 *               again on its provider session
 *   close       another conversation's agent closes its own thread
 *               (close_thread) while the window shows it; once the turn ends
 *               its agent is parked though followed (its lease ends `idle`),
 *               a read then opens it, still closed, and it is parked again;
 *               only the next message sent there starts it again
 *   provenance  a person's request in the main chat reaches a thread its
 *               agent creates; the thread follows the message back with
 *               read_turn to the person's exact words and the device they
 *               were sent from (this computer) and says them; the
 *               parent's close_thread of a thread it did not create is
 *               refused, of its own is taken, and that thread is parked
 *   rename      another conversation is renamed from its row's More menu;
 *               the gateway's list and the window say the new title, and it
 *               keeps it after a message sent there
 *   delete      another conversation is deleted from its row's More menu,
 *               once asked; the gateway refuses it as deleted, and neither
 *               list nor the window holds it
 *   swipe       with the rail on, the right panel open and every side
 *               column open, two-finger swipes left and right (one, a
 *               fling, two back to back) over the chat, the sidebar and
 *               the session list's head move no column: a sideways swipe
 *               is no way to switch columns or the panel; with the
 *               sidebar and the list folded by their keys a swipe right
 *               opens nothing; a wide table and a wide code block (its
 *               lines in a shadow root) still scroll under a swipe, and a
 *               session row's own swipe still opens and closes its actions
 *   touchId     the vault is sealed and Settings › Accounts asks for it,
 *               with a fake Touch ID on: the dialog offers Touch ID ticked
 *               and keeps the typed passphrase; the next unlock is the
 *               fingerprint alone; a dismissed prompt leaves the form with
 *               no failure and a Use Touch ID button that unlocks; changed
 *               fingerprints drop the copy and say so; the Settings switch
 *               turns it off and then rests
 *   rail        the side rail is turned on in Settings › Advanced ›
 *               Experimental and opened: the window's traffic lights stay
 *               where the host puts them above a compact rail; the top bar
 *               keeps its controls clear of them in every rail state; a message
 *               sent with the rail open is answered; closed, opened again,
 *               and turned off while open, the titlebar's controls go back to
 *               starting where the lights end; the chat's title in the pane's
 *               bar goes only between where it stood and where it lands
 *   onboarding  setup is replayed from Settings › Advanced › Setup: the host
 *               is asked once; in the setup window that ask opens, over a
 *               locked vault, no passphrase is asked for, nothing
 *               dims the desktop, and a glow fills the screen and forms the
 *               window (glow, fill, form, open, in that order) before any of
 *               the window's words can be seen; welcome's words are all
 *               there as it opens, its name plain text in the app's ink rather
 *               than typed in or painted; the pane and the light beside it
 *               are graphite (dark, no hue) on welcome, and the pane on the
 *               picker,
 *               with Get started already showing; welcome has no Back button
 *               and Close sits in the top-right corner; past it the picker
 *               has Back in the top-left corner, which returns to welcome;
 *               it lists Claude, Codex and a Local AI that is coming, never
 *               OpenCode, with each one's figure on the picture beside it;
 *               pointing at a row leaves the picture alone and clicking it
 *               turns the picture to that agent; Close then leaves setup,
 *               handing over as left, not finished; a second setup window,
 *               where Codex is not installed but can be downloaded, shows
 *               Codex as a download card with no row to pick, and no list
 *               of installed agents or Check downloads; its Download, pressed
 *               from the keyboard, fetches with the focus kept on the card,
 *               installs it, and the card becomes Codex's row, which takes
 *               the focus; on the shortcut step the host's summon, shown then
 *               hidden, teaches the step without sending a ring through the
 *               light behind the window; setup is then finished
 *               on a ready agent: the window flies into the menu bar, asking
 *               the host where its icon is, and hands over finished on that
 *               agent; the message sent in the window after it is answered
 *   notice      a review waits in another conversation, which the window does
 *               not show: the host's system notice names its tool and the
 *               conversation's title, opening it brings that conversation
 *               up with its card, and answering there takes the notice back;
 *               a second review there stays up across a reload of the
 *               window, and is taken back once another client answers it
 *   mcp         an MCP server added in Settings › Connections › MCP servers
 *               with one variable, off for new conversations: one closed
 *               row ("Local · 1 variable", its switch off); edited from its ⋯
 *               menu, the command is named and not a field, the variable
 *               rests as stored, and Replace opened and left empty saves
 *               without changing the stored value; removed once asked in
 *               its dialog, gone from the row list and config.json
 *   linking     native linking is turned on, then off, with the switch in
 *               Settings › Connections › Linked devices; each time the gateway
 *               saves config.json and starts itself again in the same
 *               process, the window comes back to the new state, and the
 *               message sent after it is answered. While it is on, the
 *               gateway is found by a DNS-SD browse on its port, and Pair a
 *               device opens a code with the window's own sign-in, in a
 *               modal with one signal orb; Escape hides it, Show code
 *               brings the same code back. A device then enters that code
 *               the way the phone does, Approve in the modal (keeping its
 *               fill on hover) makes it active and the device reads itself
 *               paired; it is listed as the owner's phone, Pair a device
 *               stays offered through the tab's polls, and Revoke drops it
 *               with a "Revoked." that a later poll clears. A second code's
 *               Cancel closes it, and leaving the tab and coming back still
 *               shows linking on
 *   drift       the gateway is stopped and started again with its staged
 *               runtime directory moved and a different agent PATH and
 *               TMPDIR, the way an app update stages a new tree and
 *               re-registers the launchd job; the next message sent in the
 *               window to the existing conversation is answered
 *   pool        native linking is turned on in Settings and two real
 *               `nessa env dial` workers pair with the gateway; a
 *               conversation created on `peer:any` is bound to one of them;
 *               its place record turned back into an older build's
 *               placement and binding files, it stays there while it
 *               serves, read from them; that worker is stopped with
 *               SIGTERM, drains and exits, and the next message sent in the
 *               window binds the conversation to the other, with new provider
 *               context, as the window's heading and the binding both say,
 *               written as one place record beside the older files;
 *               a chat started from the window in a channel whose threads
 *               use the pool runs on that worker and is told the channel's
 *               notes but neither its folder nor its attached file, both
 *               this gateway's paths; both workers stop and linking is
 *               turned off again
 *   place       native linking is turned on and a real `nessa env dial`
 *               worker pairs; a channel naming a computer not connected is
 *               refused channel_place_unavailable; in the channel's settings
 *               New threads runs on Computer, the worker, chosen from the
 *               computers listed; a chat started in the channel from the
 *               window (⌘N, Send) is answered with its lease on that worker
 *               and the window says where it runs; the channel goes back
 *               here, the worker stops and linking is turned off again
 *   tunnel      native linking and then the web tunnel are turned on in
 *               Settings, each restarting the gateway, which starts a
 *               stand-in cloudflared (NESSA_TUNNEL_PROGRAM) and shows its
 *               address once ready; the gateway is stopped and started
 *               again and takes the same cloudflared back, at the same
 *               address; a real `nessa env dial` worker pairs, then dials
 *               only through the tunnel (its direct address refuses), and a
 *               conversation placed on it is answered; turning the tunnel off
 *               ends the cloudflared, and linking is turned off again
 *   launch      the window is reloaded while the host is still starting the
 *               gateway: the host holds the window's read, the startup
 *               screen never covers it, and once the gateway is up the
 *               window lists the conversation and the message sent after
 *               it is answered
 *   github      GitHub is connected in Settings › Connections › Accounts
 *               against a fake GitHub on this machine: the row shows the
 *               code GitHub handed out and where to enter it, and once the
 *               fake approves it shows the account's picture (GitHub's
 *               avatar host answered by the page), its login and
 *               "Connected", its actions out only under the pointer or
 *               keyboard focus; the sign-in is a private
 *               file the gateway never logs; agents and shell commands have
 *               no Nessa GitHub token or git helper while connected or after
 *               Disconnect. A source-authorized gateway tool reads a fixed
 *               repository with the connected token. The audit holds each
 *               sign-in step and no token
 *   login       an agent's login is pasted into Settings › Connections ›
 *               Accounts › Saved logins: the row says only "Saved", its
 *               field is cleared, and the gateway says it is saved. A
 *               paired worker whose owner lets it take logins
 *               (`env grant --logins`) starts the agent without the login
 *               while its "Use my saved logins here" switch in Devices is
 *               off, and with it in the agent's login variable once it is
 *               on; the gateway's logs and records never hold the login,
 *               and its records say it was withheld, then sent. The switch
 *               and the login are removed again after
 *   voice       Talk while Nessa is locked offers Unlock beside the notice
 *               and then starts the call; with no key saved it fails with
 *               an Add key button beside
 *               the notice; the key is saved through its dialog (Settings
 *               then says it is saved, with no key field), against a fake
 *               realtime provider on this machine: the key
 *               is never logged or audited, only the gateway asks the
 *               provider for a call. Talk opens a call on the composer
 *               (a glow on its edge, "Listening", and the speaker, microphone
 *               and End call buttons); the chat so far is handed to the voice; the
 *               person's spoken words, fillers and a repeated word dropped,
 *               are sent as their message and answered, and the reply is
 *               handed back to the voice to talk about; muting the
 *               microphone clears what it half heard; a stop the person
 *               talked over (its response cancelled) stops nothing; a
 *               follow-up said while the agent works is sent queued, and
 *               asked to, the voice stops the running turn; once the agent
 *               stops work the voice is told what it said once and the
 *               call listens again; words said or heard stay on one
 *               line that keeps its newest words; with another chat opened in the
 *               call's pane the call goes on, its pill shows over the
 *               front window, what is said still reaches the agent and
 *               the reply the voice, a stop said there stops the agent's
 *               turn, and the chat's name on the pill
 *               brings the chat back and puts the pill away; Talk in
 *               another chat asks before it ends the open call, Cancel
 *               leaves it and Talk here moves it; a call that drops
 *               with its chat in no pane is said in the window; End call
 *               closes the call and Talk is back
 *   upgrade     the gateway is stopped, its conversation metadata turned
 *               back into the file an older build wrote (version 6, or 7
 *               when a newer table holds rows) and every place record into
 *               an older build's placement and binding files, and started
 *               again: it upgrades the file to the current version, lists every
 *               conversation it listed before, and the message sent in the
 *               window after it is answered
 *   channel     a channel is made from the sidebar's +, asked in the
 *               transcript: named, a folder chosen in the host's picker that
 *               agents may not work in (the root) refused at its question,
 *               then one made for it; its model kept, notes typed, and a file
 *               attached from the host's picker, then a file and a folder
 *               dragged onto the window (the whole flow lit as the drop
 *               target while dragged over, each listed once with its kind,
 *               the hint on one line, the count as 4/5); Create opens a new
 *               session's home in it, whose first message runs in the made
 *               folder with the notes in the agent's system prompt (Claude).
 *               A channel row that is not selected keeps its hover fill
 *               while the pointer moves onto its settings beside it.
 *               A second channel works in a folder chosen in the picker, and
 *               its first message runs there; the gateway lists both, with
 *               their folders, and each conversation in its own. When the
 *               gateway reports Boat connected, the first then moves its new
 *               threads and its main agent to Boat and back from its settings
 *               dialog: new threads alone keep the main chat, a moved main
 *               agent names a fresh one and the one talked to stays in the
 *               channel. Without a connected Boat those Boat checks skip
 *   huddle      a turn shows the test server's talk board, the host side of
 *               a voice app: its frame is allowed the microphone, it opens in
 *               the chat's panel as one tab however often it asks (its card a
 *               row), its first message is reviewed and answered Always allow
 *               this app, and its next message is sent with no review;
 *               then Revoke under Settings › Access › Apps you always allow
 *               takes the trust back, out of the gateway's file too
 *   composer    the composer's row names the model short, with thinking,
 *               the hairline and Send at its end; the model menu's provider
 *               tiles are square; stacked three high in a short window, a
 *               pane you are not in with nothing typed folds its composer to
 *               a circle, one with a draft keeps its line, and pressing a
 *               circle puts the caret in that pane's field
 *   menu        the pane's ⋯ menu opens with every row leading with its
 *               icon at the menu recipe's row height; in the overview's
 *               filter the chosen scope and span each show their check
 *               before their number, the numbers stand in one column, and
 *               pressing a number picks that choice and closes the menu;
 *               the message sent in the window after it is answered
 *   paste       an image is pasted into the composer as a copied screenshot
 *               is: it shows as a tile and types nothing; sent with the
 *               message, the composer lets it go, the conversation holds it
 *               and the agent says it was handed exactly that image; the
 *               person's message draws it, read back from the conversation
 *   attach      a text file and an image are picked with the composer's
 *               Attach button: the file shows by its name, the image as its
 *               picture; sent with the message, the conversation holds the
 *               image as an image and the file as an upload under its name,
 *               the agent is handed the image and finds the file's bytes in
 *               its workspace, and the person's message draws both. The
 *               window runs under the packaged app's connect-src, so a read
 *               of the attachments' bytes the app would block fails here too
 *   trace       the agent makes three calls, one failing, then answers: the
 *               gateway's view keeps each call's start and end time and the
 *               input it was made with; the window draws them as one folded
 *               line, "Ran 3 tools", that opens to a row per call, each with
 *               its status and a bar on the shared time axis (at least as
 *               wide as it is tall, ending inside the axis), and a call's
 *               card shows its input. While the run is still at work it
 *               alone says so: its line reads "Working", open or folded, the
 *               call at work is named once, and no live row repeats it
 *   transcriptView  Compact is chosen from the pane's "…" menu, then the
 *               trace turn is sent: while it works the transcript draws no
 *               run, only one live row that reads "Working" and names no
 *               call; once it ends, the reply shows and no run does, while
 *               the gateway still keeps every call. Compact outlasts a
 *               reload; Detailed, chosen again, draws the folded run
 *   ask         an agent asks its owner one decision (`ask_owner`) in a
 *               conversation the window does not show: the list row carries
 *               it, the row waits on the person, and the host's notice says
 *               the question and names the conversation, all without the
 *               window leaving the conversation it shows. Opening the
 *               notice brings up its card, with each choice and the one
 *               recommended; tapping the other choice sends its label as
 *               the person's message, which the agent answers, and the
 *               card, the row's ask and the notice all go
 *   accessRequest  an agent asks to raise its chat's access (`request_access`):
 *               the row's ask is Nessa's own question and choices with the
 *               agent's reason, and Not now is the one recommended. The red
 *               card says them and lists nothing yet; Approve raises the chat
 *               to Auto by the person's own access command, and the agent's
 *               turn on their Approve begins at a different permission mode
 *               than the asking turn ran at (each turn says its own); the card stays to
 *               list the three lines of what was allowed, and the row's ask
 *               goes. The chat is set back to Ask after
 *
 * Meanwhile `--parallel` background conversations take turns through the
 * gateway (every third one a watch), and `--followers` other clients follow
 * every conversation's events, the main view and the list
 * (`lib/stress-followers.mjs`), stopping and following again as they go, and
 * across every restart. At the end each follower must have applied every
 * conversation's events exactly once and in order. With `--parks` above 0 the
 * gateway parks every idle conversation nobody follows after
 * `PARK_IDLE_SECONDS`, so asides, threads and background conversations are
 * parked and started again as the run goes. At the end every conversation
 * must be at rest, every turn ended as
 * planned, and nothing denied automatically or hidden behind a display limit.
 *
 * Build the gateway first (`cargo build -p nessa-server`) and the peer
 * example the `peer` step enrolls
 * (`cargo build -p nessa-client-core --example read_only_sync`). How to add
 * a step is in CODING_STANDARDS.md, "Conversation stress".
 *
 *   pnpm test:e2e:stress                        # the pull request's run
 *   pnpm test:e2e:stress -- --rounds 60 --parallel 12 --seed 7
 *   NESSA_STRESS_ROUNDS=60 pnpm test:e2e:stress # the same, from the environment
 */
import catalog from "../../../crates/nessa-sdk/data/models.json" with { type: "json" }
import { randomUUID } from "node:crypto"
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  renameSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { spawn, spawnSync } from "node:child_process"
import { tmpdir } from "node:os"
import { basename, dirname, isAbsolute, join } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"

import { readWithReviews } from "../../../scripts/conversation-smoke/reviews.mjs"
import { tinyPng } from "../../../scripts/conversation-smoke/tiny-png.mjs"
import { MODELS, repoRoot } from "../../../scripts/mcp-test-server/local-gateway.mjs"
import { turnEnded } from "../../../scripts/mcp-test-server/evidence.mjs"
import {
  SAYS,
  STREAM_HOLD_MS,
  STREAM_WORDS,
  STRESS,
  TABLE_HOLD_MS,
  TABLE_LINES,
  RUN_PRINTS,
  RUN_COMMAND,
  CODE_LINES,
  MATH_LINES,
  TRACE_CALLS,
  TRACE_PAUSE_MS,
  WATCH_CALLS,
  ASK,
  ACCESS,
  RAISE_REVIEWS,
  streamWord,
  stressScenario,
} from "../../../scripts/mcp-test-server/scenarios/stress.mjs"
import { appConnectPolicy, assertPolicyHolds } from "./lib/app-policy.mjs"
import { appFrame } from "./lib/apps.mjs"
import { browse } from "./lib/mdns-probe.mjs"
import { noteLiveMountResourceAbort, openPage, withEngines } from "./lib/browser.mjs"
import { CannotRun, log } from "./lib/cli.mjs"
import { gatewayHost, STARTING } from "./lib/fake-host.mjs"
import {
  connectClient,
  panelTarget,
  scriptedLaunch,
  startGatewayStack,
  webhookNoAttempt,
  waitFor,
} from "./lib/gateway-stack.mjs"
import { main } from "./lib/run.mjs"
import {
  closeAcceptance,
  parkingEvidence,
  readFailureView,
  leaseReopenedAfter,
} from "./lib/stress-close.mjs"
import { writeView } from "./lib/scripted-evidence.mjs"
import {
  composerControls,
  css,
  keys,
  names,
  offeredLabel,
  overviewFilterKeys,
  selectorFor,
  storage,
} from "./lib/selectors.mjs"
import {
  FAKE_ACCOUNT,
  FAKE_AVATAR,
  GITHUB_AVATARS,
  startFakeGithub,
  disposableGithubEnvironment,
} from "./lib/fake-github.mjs"
import { startFakeRealtime } from "./lib/fake-realtime.mjs"
import { freePort } from "./lib/server.mjs"
import { Follower } from "./lib/stress-followers.mjs"
import { FAKE_CHATGPT_ACCOUNTS, startFakeOpenai } from "./lib/fake-openai.mjs"
import {
  asEarlierPlaces,
  asOlderBuild,
  metadataFile,
  placeOf as placeRecordOf,
  schemaVersion,
} from "./lib/stress-upgrade.mjs"
import { nextSshHosts, writeFakeSsh, writeSshConfig } from "./lib/stress-ssh-hosts.mjs"
import {
  pairedCatalogue,
  preparePairedCatalogue,
} from "./lib/stress-paired-catalogue.mjs"
import {
  openStressConversation,
  selectedStressConversation,
  selectStressModel,
  clearStarredModels,
  chatIdentityTexts,
} from "./lib/stress-conversation.mjs"
import { phoneFilesVerifier, uploadedInputFiles } from "./lib/stress-input-files.mjs"
import { observeGatewayRestarts } from "./lib/stress-restart.mjs"
import {
  contextAudit,
  nativeContextEvents,
  nativeContextExists,
  newContextEvidence,
} from "./lib/stress-native-context.mjs"
import {
  observeReviewAnswers,
  observeAccessRefusals,
  reviewAnswerAcknowledged,
} from "./lib/stress-review-answers.mjs"
import {
  hasAgentSource,
  parentRestarted,
  parentStopCheckpoint,
} from "./lib/stress-parent-stop.mjs"
import { KNOBS, describe, knobsFrom, selectedPlan, random } from "./lib/stress-plan.mjs"
import { flatSurfaceFailures } from "./lib/workspace.mjs"
import { startWorker } from "./lib/stress-workers.mjs"
import { vmBackgroundFixture } from "./lib/stress-vm-background.mjs"
import {
  stressProfile,
  selectStressFixture,
  requireStressCapabilities,
  elevatedAccessAfter,
} from "./lib/stress-profile.mjs"
import { raisedTurn } from "./lib/raise-evidence.mjs"

const INSTANCE = "conversation-stress"
/** The conversation metadata version this build writes (`schema.sql`'s last line). */
const CURRENT_METADATA_VERSION = Number(
  /PRAGMA user_version = (\d+);\s*$/.exec(
    readFileSync(
      new URL(
        "../../../crates/nessa-conversation/src/infrastructure/schema.sql",
        import.meta.url,
      ),
      "utf8",
    ),
  )[1],
)
/**
 * The environment that places a conversation on any of the gateway's paired
 * workers (`POOL` in crates/nessa-conversation/src/application/placement.rs).
 */
const POOL = "peer:any"
/**
 * The variable each agent is handed its saved login in on a worker: the
 * first of `login_variables` (crates/nessa-protocol/src/lease.rs).
 */
/** The variable naming the file a ChatGPT token is handed to Codex in. */
const CHATGPT_TOKEN_FILE = "NESSA_CHATGPT_TOKEN_FILE"
/** Whether `text` holds any of `secrets`. */
const secretsIn = (text, secrets) => secrets.some((secret) => text.includes(secret))

const LOGIN_VARIABLES = {
  claude: "CLAUDE_CODE_OAUTH_TOKEN",
  codex: "CODEX_API_KEY",
  opencode: "OPENCODE_API_KEY",
}

const meta = {
  name: "conversation-stress",
  summary:
    "many rounds in one conversation through the gateway and the window, with access switches, stops, failures, restarts, threads and side conversations",
  defaults: { engine: "chromium", layout: "columns" },
  options: {
    agent: { type: "string", default: "claude" },
    model: { type: "string" },
    accessScenario: { type: "string", default: "full" },
    evidence: { type: "string" },
    only: { type: "string" },
    ...Object.fromEntries(Object.keys(KNOBS).map((name) => [name, { type: "string" }])),
  },
  help: `
Usage: node verification/desktop/scripts/conversation-stress.mjs [options]

Needs: the gateway built (cargo build -p nessa-server; or MCP_LIVE_NESSA)
and the peer example (cargo build -p nessa-client-core --example read_only_sync). No
harness and no sign-in: the gateway runs scripted-agent.mjs with the stress
scenario (scripts/mcp-test-server/scenarios/stress.mjs). It starts its own
gateway; --url is not used.

Numbers (a flag, else NESSA_STRESS_<NAME>, else the default):
${Object.entries(KNOBS)
  .map(
    ([name, knob]) =>
      `  --${name.padEnd(9)} ${String(knob.default).padStart(3)}  ${knob.what}`,
  )
  .join("\n")}

Options:
  --agent claude|codex  the harness protocol (default: claude)
  --model <id>          explicit scripted model; defaults to the shared agent fixture
  --accessScenario full|ask-only
                        full requires advertised Ask/Auto/Full; ask-only requires
                        only Ask and verifies Full refusal, never Full success.
                        Codex examples: --model gpt-6-astra for full;
                        --model gpt-5.6-terra --accessScenario ask-only on Linux.
  --evidence <dir>      write the plan, acp.jsonl, mcp.jsonl, gateway.log and views/
  --only <kinds>        explicit comma-separated selected-step qualification; keeps
                        all knobs/deadlines and reports selected scope, not a full run`,
}

/**
 * How long a conversation sits idle, unwatched, before the gateway parks its
 * agent in this run (`limits.agentIdleSeconds`). Short, so the run sees it;
 * the gateway looks four times within it.
 */
const PARK_IDLE_SECONDS = 3

/**
 * How many chunks each answer of the `long` step streams, and how many bytes
 * each carries. One answer stays within the 16 KB of text the gateway's view
 * shows of a message, which clips a longer one; the saved history keeps all
 * of every answer.
 */
const LONG_CHUNKS = 40
const LONG_CHUNK_BYTES = 300
/** Chunks of an `oversized` answer: about 74 KB, past one view's 60 KB frame. */
const OVERSIZED_CHUNKS = 240
/**
 * How many other conversations the `fresh` step creates and sends to at the
 * same moment as the window's own first message: more than the sixteen
 * command owners the gateway once allowed at a time, which refused the
 * seventeenth as `conversation_storage_unavailable` instead of queueing it.
 */
const FRESH_BURST = 24
/** The clients that burst is spread over, each well within one socket's 16 requests at once. */
const FRESH_CLIENTS = 3
/**
 * The `launch` step's host stays "starting" this long after the window is
 * reloaded, well under the host's hold (`STARTING.waitMs`). A refused window
 * covers itself with the startup screen at once, which is what the step
 * checks; `LAUNCH_LISTED_MS` only bounds how long listing may take once the
 * gateway is up.
 */
const LAUNCH_STARTING_MS = 3_000
const LAUNCH_LISTED_MS = 10_000
/** A slow launch's hold: short, so outlasting it costs the run little. */
const SLOW_LAUNCH_HOLD_MS = 1_000
/**
 * How much longer opening the `long` conversation again may take than
 * opening one of a single long turn, both closed first so their whole saved
 * history is replayed: half a second of noise, and a little for each turn.
 * Measured against the short one in the same run, so a loaded machine slows
 * both. On a 4-core machine, replayed in pages each turn adds about 15 ms;
 * replayed fact by fact (#1006) each added about 60 ms, past this.
 */
const reopenGrowthMs = (turns) => 500 + 20 * turns

const fold = (text) =>
  String(text ?? "")
    .replace(/\s+/g, " ")
    .trim()

/** An error with its code and cause, which a client failure keeps apart from its message. */
const explain = (error) =>
  [error?.message, error?.code, error?.cause?.message ?? error?.cause?.code]
    .filter(Boolean)
    .join(" · ")

const dock = (page) => page.locator(css.conversationDock)

/**
 * The composer says nothing of its own: what a control has to say is its
 * tooltip (`ComposerNote`), so the docked composer holds no element but the
 * draft and its row, and no status line anywhere in it.
 */
async function composerSaysNothing(page, when, failures) {
  const said = await dock(page)
    .locator(composerControls.said)
    .evaluateAll((nodes) => nodes.map((node) => node.outerHTML.slice(0, 160)))
    .catch(() => [])
  if (said.length)
    failures.push(
      `the composer said something of its own ${when}: ${JSON.stringify(said)}`,
    )
}

async function pageText(page) {
  return fold(
    await page
      .locator(css.workspace)
      .innerText()
      .catch(() => ""),
  )
}

/** Waits up to `ms` for the window to show `text`. */
const shows = (page, text, ms = 30_000) =>
  waitFor(async () => (await pageText(page)).includes(fold(text)), ms).then(Boolean)

/**
 * Reads until `ready` holds for the turn `at` names: a message index, or
 * `{ text }`, the turn the person sent as `text` (every text in a run is its
 * own). A read keeps only the latest turns and the agent's own turns can land
 * in between, so a sent turn is found by its text, never its position.
 */
async function untilTurn(run, conversationId, at, ready, seconds = 60) {
  const end = Date.now() + seconds * 1000
  let view
  let turn
  do {
    try {
      view = await readWithReviews(run.client, conversationId)
      turn =
        typeof at === "number"
          ? view.messages[at]
          : view.messages.find((each) => each.userText === at.text)
      if (turn && ready(turn, view)) return { view, turn }
    } catch (error) {
      // A read across a restart fails; the next one is on the new client.
      log(`read ${conversationId}: ${error.message}`)
    }
    await sleep(200)
  } while (Date.now() < end)
  return { view, turn }
}

/**
 * Reads `conversationId` until `ready` holds for its whole view, which may
 * have no turns: a conversation whose first submit was refused has none.
 */
async function untilView(run, conversationId, ready, seconds = 60) {
  const end = Date.now() + seconds * 1000
  let view
  do {
    try {
      view = await readWithReviews(run.client, conversationId)
      if (ready(view)) return { view }
    } catch (error) {
      log(`read ${conversationId}: ${error.message}`)
    }
    await sleep(200)
  } while (Date.now() < end)
  return { view }
}

const ended = (turn) => turnEnded(turn.status)

/**
 * The `swipe` step's swipes, with the rail on and the wide table drawn in
 * `message` (its scroller `port`) beside the wide code block `code`: a
 * sideways two-finger swipe moves no side column and not the right panel,
 * whether everything is open or the sidebars are folded, while the table,
 * the code and a session row still take their own swipes.
 */
async function sidewaysSwipes(run, step, message, port, code, failures) {
  const page = run.page
  const columns = () =>
    page.evaluate(
      ({ frame, workspace }) => ({
        rail: document.querySelector(frame)?.dataset.rail,
        panel: document.querySelector(workspace)?.dataset.panel,
        sidebar: document.querySelector(workspace)?.dataset.sidebar,
        list: document.querySelector(workspace)?.dataset.list,
      }),
      { frame: css.workspaceWindow, workspace: css.workspace },
    )
  // Every column open to start: the sidebar and the list by their keys.
  const start = await columns()
  if (start.sidebar !== "open") await page.keyboard.press(keys.toggleSidebar)
  if (start.list !== "open") await page.keyboard.press(keys.toggleSessionList)
  // The right panel open too: a swipe moves none of them.
  if (start.panel !== "open") await page.keyboard.press(keys.togglePanel)
  const open = { panel: "open", sidebar: "open", list: "open" }
  const at = async (want) => {
    const now = await columns()
    return Object.keys(want).every((each) => now[each] === want[each])
  }
  if (!(await waitFor(() => at(open), 5_000))) {
    failures.push(`the side columns did not all open: ${JSON.stringify(await columns())}`)
    return
  }
  // The rail as the window drew it beside the open panel.
  const all = { ...open, rail: (await columns()).rail }
  // Two fingers over the reply's words: `x` positive is the fingers
  // moving left (the Mac's natural scrolling, which Chromium assumes).
  // On Chromium the events go in through the browser's input with their
  // own times 16 ms apart, as a trackpad sends them, so a slow machine
  // cannot split one gesture in two; elsewhere they are sent as they
  // come. The pause after outlasts the gesture's end, so each is its own
  // swipe.
  const words = message.locator("p").first()
  const input =
    run.engine === "chromium" || run.engine === undefined
      ? await page.context().newCDPSession(page)
      : null
  let clock = Date.now() / 1000
  const swipe = async (over, x, y = 0, events = 8) => {
    await over.scrollIntoViewIfNeeded()
    const box = await over.boundingBox()
    const point = box && {
      x: Math.round(box.x + Math.min(box.width / 2, 120)),
      y: Math.round(box.y + Math.min(box.height / 2, 12)),
    }
    const onIt =
      point &&
      (await over.evaluate(
        (element, { x, y }) =>
          element.contains(document.elementFromPoint(x, y)) &&
          document
            .elementFromPoint(x, y)
            ?.closest(".workspace-chat, .workspace-sidebar, .workspace-list") !== null,
        point,
      ))
    if (!onIt) {
      failures.push(
        `nothing to swipe over in the chat or a side column at ${JSON.stringify(point)}`,
      )
      return
    }
    await page.mouse.move(point.x, point.y)
    clock = Math.max(clock + 1, Date.now() / 1000)
    // `x` is each event's travel, or a list of them for a gesture whose
    // events differ (a fling's momentum dying down, then the next swipe).
    const xs = Array.isArray(x) ? x : Array.from({ length: events }, () => x)
    if (!input && Array.isArray(x))
      // Elsewhere a gesture with no pause in it goes in from the page, one
      // wheel event a frame, as a trackpad sends them: a slow round trip
      // from the script cannot open a pause, and the window draws between
      // events as it would.
      await page.evaluate(
        async ({ point, xs, y }) => {
          const target = document.elementFromPoint(point.x, point.y)
          for (const deltaX of xs) {
            await new Promise((done) => requestAnimationFrame(done))
            target?.dispatchEvent(
              new WheelEvent("wheel", {
                bubbles: true,
                cancelable: true,
                clientX: point.x,
                clientY: point.y,
                deltaX,
                deltaY: y,
              }),
            )
          }
        },
        { point, xs, y },
      )
    else
      for (const [i, each] of xs.entries()) {
        if (input)
          await input.send("Input.dispatchMouseEvent", {
            type: "mouseWheel",
            x: point.x,
            y: point.y,
            deltaX: each,
            deltaY: y,
            timestamp: clock + i * 0.016,
          })
        else await page.mouse.wheel(each, y)
      }
    await page.waitForTimeout(400)
  }
  // Each swipe has waited out its gesture (`swipe`'s pause), so the
  // columns read now are the ones it left.
  const expect = async (label, want) => {
    const now = await columns()
    if (!(await at(want)))
      failures.push(
        `${label}: the columns are ${JSON.stringify(now)}, not ${JSON.stringify(want)}`,
      )
  }
  // Over the wide table the swipe is the table's: it scrolls, nothing folds.
  // First, with every column open, where the reply is narrowest.
  const hidden = await port.evaluate((each) => each.scrollWidth - each.clientWidth)
  if (hidden <= 1) failures.push(`the wide table does not scroll (${hidden} px hidden)`)
  const scrolled = await port.evaluate((each) => each.scrollLeft)
  await swipe(port, 15)
  await expect("a swipe over the wide table", all)
  if ((await port.evaluate((each) => each.scrollLeft)) <= scrolled)
    failures.push("a swipe over the wide table did not scroll it")
  // Over a wide code block the same. The highlighter draws its lines in a
  // shadow root, where the wheel's target is only the root's host and its
  // scroller is out of reach of the target's own parents.
  const codeScroll = () =>
    code.evaluate((block) => {
      const walk = (root) => {
        for (const each of root.querySelectorAll("*")) {
          const overflow = getComputedStyle(each).overflowX
          if (
            each.scrollWidth > each.clientWidth + 1 &&
            (overflow === "auto" || overflow === "scroll")
          )
            return each.scrollLeft
          const inner = each.shadowRoot && walk(each.shadowRoot)
          if (inner !== null && inner !== undefined) return inner
        }
        return null
      }
      return walk(block)
    })
  // The highlighter draws a block's lines only once it is in view.
  await code.scrollIntoViewIfNeeded()
  if (!(await waitFor(async () => (await codeScroll()) !== null, 15_000)))
    failures.push("the wide code block does not scroll")
  else {
    const before = await codeScroll()
    await swipe(code, 15)
    await expect("a swipe over the wide code block", all)
    if ((await codeScroll()) <= before)
      failures.push("a swipe over the wide code block did not scroll it")
  }
  // Over the reply's words a swipe either way moves nothing: one swipe, a
  // fling, two back to back, and one picking up as the last one's momentum
  // fades. The rail and the right panel stay too.
  const fling = [15, 15, 15, 15, 15, 15, 8, 4, 2, 1, 0.5]
  const fading = [15, 15, 15, 15, 15, 15, 12, 10, 8, 6, 5, 4, 3]
  const picking = [4, 8, 14, 18, 18, 18, 18, 18]
  const neg = (xs) => xs.map((each) => -each)
  for (const [label, x, events] of [
    ["a swipe left", 15, 8],
    ["a fling left", 15, 30],
    ["two swipes left back to back", [...fling, ...fling]],
    ["a second swipe left as the first one's momentum fades", [...fading, ...picking]],
    ["a swipe right", -15, 8],
    ["two swipes right back to back", neg([...fling, ...fling])],
  ]) {
    await swipe(words, x, 0, events)
    await expect(`${label} over the chat`, all)
  }
  // Over the side columns the same: the sidebar and the list's head.
  const inSidebar = page.locator(css.overviewEntry).first()
  const listHead = page.locator(`${css.sessionList} .desktop-column-title`).first()
  for (const [label, over] of [
    ["the sidebar", inSidebar],
    ["the session list", listHead],
  ]) {
    await swipe(over, 15)
    await expect(`a swipe left over ${label}`, all)
    await swipe(over, -15)
    await expect(`a swipe right over ${label}`, all)
  }
  // A session row's own two-finger swipe is still the row's: left uncovers
  // its Archive and Delete, right covers them again, and no column moves.
  // The row ends a wheel swipe after 140 ms without an event by its own
  // clock, which a slow round trip from here would open, so each swipe
  // goes in from the page as one burst, as session-rows.mjs sends them.
  const row = page.locator(selectorFor.sessionListRowFor(run.main.id))
  const tray = () => row.locator("xpath=..").getAttribute("data-swipe")
  const rowSwipe = async (deltaX) => {
    await row.scrollIntoViewIfNeeded()
    await row.hover()
    await row.evaluate((element, deltaX) => {
      for (let i = 0; i < 6; i += 1)
        element.dispatchEvent(
          new WheelEvent("wheel", { deltaX, bubbles: true, cancelable: true }),
        )
    }, deltaX)
    await page.waitForTimeout(400)
  }
  await rowSwipe(40)
  await expect("a swipe left over a session row", all)
  if (!(await waitFor(async () => (await tray()) === "open", 3_000)))
    failures.push(`a swipe left over a session row left it ${await tray()}, not open`)
  await rowSwipe(-40)
  await expect("a swipe right over an open session row", all)
  if (!(await waitFor(async () => (await tray()) === null, 3_000)))
    failures.push(`a swipe right over an open session row left it ${await tray()}`)
  // Folded by their keys, the sidebar and the list stay folded under a
  // swipe right, as they once would have opened.
  await page.keyboard.press(keys.toggleSessionList)
  await page.keyboard.press(keys.toggleSidebar)
  const none = { ...all, sidebar: "closed", list: "closed" }
  if (!(await waitFor(() => at(none), 5_000)))
    failures.push(
      `the sidebars did not fold by their keys: ${JSON.stringify(await columns())}`,
    )
  else {
    await swipe(words, -15)
    await expect("a swipe right with the sidebars folded", none)
    await swipe(words, -15, 0, 30)
    await expect("a fling right with the sidebars folded", none)
  }
  await page.keyboard.press(keys.toggleSidebar)
  await page.keyboard.press(keys.toggleSessionList)
  if (!(await waitFor(() => at(all), 5_000)))
    failures.push(
      `the sidebars did not open by their keys: ${JSON.stringify(await columns())}`,
    )
  if (run.evidence)
    await page.screenshot({ path: join(run.evidence, "swipe-all-open.png") })
  // The panel as the step found it, for the steps after.
  if (start.panel !== "open") await page.keyboard.press(keys.togglePanel)
  // The conversation still takes a message.
  await steps.reply(run, { ...step, extra: " after swiping" }, failures)
}

/**
 * Turns the side rail preview on or off where a person does: Settings ›
 * Advanced › Experimental. Leaves Settings, back in the workspace.
 */
async function setSideRail(page, on, failures) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await page.locator(css.settingsCategory, { hasText: names.advanced }).click()
  // A category with one tab draws no tab list.
  const tab = page.locator(css.settingsTab, { hasText: names.experimental })
  if (await tab.count()) await tab.click()
  const toggle = page.locator(`${selectorFor.settingRow("side-rail")} [role='switch']`)
  if ((await toggle.getAttribute("aria-checked")) !== String(on)) await toggle.click()
  const switched = await waitFor(
    async () => (await toggle.getAttribute("aria-checked")) === String(on),
    5_000,
  )
  if (!switched) failures.push(`the side rail's switch did not turn ${on ? "on" : "off"}`)
  const drawn = await waitFor(
    async () => (await railLayout(page)).rail === (on ? "open" : "off"),
    10_000,
  )
  if (!drawn) {
    const now = await railLayout(page)
    failures.push(
      on && now.rail === "closed"
        ? "the window had no room for the side rail"
        : `under Settings the rail is ${now.rail}, not ${on ? "open" : "off"}`,
    )
  }
  await page.getByRole("button", { name: names.leaveSettings }).first().click()
  await page.waitForSelector(css.settings, { state: "detached", timeout: 10_000 })
  const shown = await waitFor(
    async () => (await page.locator(css.sideRailToggle).count()) === (on ? 1 : 0),
    10_000,
  )
  if (!shown) failures.push(`the side rail's toggle is ${on ? "missing" : "still shown"}`)
}

/**
 * The expanded right panel is the chat in front's: its own tabs, or with
 * nothing open its note, no tab strip for the chats and no toggles for the
 * columns it covers. ⌘N docks it before the new session opens, so the
 * session is in front, in sight — never under the panel. The chat is
 * brought back, docked.
 */
async function expandedForItsChat(run, failures) {
  const page = run.page
  if (await chatTabs(page)) failures.push("the window draws a tab strip for the chats")
  const panel = page.locator("#workspace-panel")
  if (
    !(await panel.locator("[data-panel-empty], .workspace-panel-tabs").count()) ||
    ((await panel.locator("[data-panel-empty]").count()) &&
      (await panel.innerText()).trim() !== "Nothing open")
  )
    failures.push(
      `the expanded panel shows neither its tabs nor its note: "${await panel.innerText()}"`,
    )
  if (await page.locator(`${css.titlebar} [aria-controls="workspace-sidebar"]`).count())
    failures.push("the expanded panel's titlebar offers the sidebar it covers")
  await page.keyboard.press(keys.newSession)
  const docked = await waitFor(
    async () => (await page.locator("#workspace-panel[data-maximized]").count()) === 0,
    2000,
  )
  if (!docked) failures.push("⌘N left the panel expanded over the new session")
  const home = page.locator(`${css.focusedPane} ${css.paneHome}`)
  await home.locator(css.field).waitFor({ timeout: 15_000 })
  if (await home.evaluate((node) => node.closest("[inert]") !== null))
    failures.push("the new session opened under the panel")
  await openStressConversation(page, run.main.id, run.main.title)
}

/**
 * "nessa Studio" belongs to whatever column holds the bottom-left corner, at
 * any width: with the sidebar folded the session list still holds it and the
 * words stay; only with the panes alone at the window's edge are they gone.
 * Leaves the columns as it found them.
 */
async function studioWithTheCorner(page, failures) {
  const shown = () =>
    page.evaluate(
      () =>
        getComputedStyle(document.querySelector(".side-rail-studio") ?? document.body)
          .visibility === "visible",
    )
  const column = (controls) =>
    page.locator(`${css.titlebar} button[aria-controls="${controls}"]`)
  const list = page.locator(
    ["Hide", "Show"]
      .map((verb) => `${css.titlebar} button[aria-label^="${verb} Session List"]`)
      .join(", "),
  )
  const sidebar = column("workspace-sidebar")
  // Hidden, it fades first and turns invisible at the fade's end: a "shown"
  // read too soon after a change would see the fade. Settle, then read.
  const expect = async (want, what) => {
    await page.waitForTimeout(600)
    if (!(await waitFor(async () => (await shown()) === want, 2000)))
      failures.push(`"nessa Studio" is ${want ? "missing" : "shown"} ${what}`)
  }
  const open = async (button) =>
    (await button.count()) > 0 && (await button.getAttribute("aria-expanded")) === "true"
  const set = async (button, want) => {
    if ((await button.count()) > 0 && (await open(button)) !== want) await button.click()
  }
  // Away from the window's left edge, where the pointer would peek the sidebar out.
  const size = page.viewportSize()
  if (size) await page.mouse.move(size.width / 2, size.height / 2)
  const was = { sidebar: await open(sidebar), list: await open(list) }
  await set(sidebar, true)
  await expect(true, "beside the open sidebar")
  await set(sidebar, false)
  if ((await list.count()) > 0) {
    await set(list, true)
    await expect(true, "with the session list holding the corner")
    await set(list, false)
  }
  await expect(false, "over the panes alone")
  await set(list, was.list)
  await set(sidebar, was.sidebar)
}

/** The macOS traffic lights end at x=76 (`traffic_lights.rs`). */
const LIGHTS_END = 76

/**
 * The folded sidebar parks once its slide has ended (#1177): hidden from
 * assistive tech from the fold, then inert, unpainted and skipped by style
 * (`content-visibility: hidden`) in one change, so its rows are never
 * restyled on a frame of the fold. Opened again, it is reachable at once.
 */
/**
 * Watches every frame of a fold, a split or a close for a pane that changes
 * how it looks while it moves: a composer losing its glass blur or shadow, or
 * a laid-out scene skipped for a frame. Either flashed on every fold and drop
 * and read as the window flickering. Call it before the key, and the function
 * it returns after the motion has landed.
 */
async function lookHolds(page, label) {
  await page.evaluate(() => {
    const seen = []
    let on = true
    const tick = () => {
      if (!on) return
      for (const composer of document.querySelectorAll(".desktop-composer")) {
        const style = getComputedStyle(composer)
        if (style.backdropFilter === "none" || style.boxShadow === "none")
          seen.push(`composer ${style.backdropFilter} / ${style.boxShadow}`)
      }
      for (const scene of document.querySelectorAll(
        ".desktop-night-scene[data-scene-complete]",
      ))
        if (getComputedStyle(scene).contentVisibility === "hidden")
          seen.push("a laid-out scene skipped")
      requestAnimationFrame(tick)
    }
    requestAnimationFrame(tick)
    window.__stressLook = () => {
      on = false
      return seen
    }
  })
  return async (failures) => {
    await page.waitForTimeout(700)
    const seen = await page.evaluate(() => window.__stressLook?.() ?? [])
    if (seen.length)
      failures.push(
        `${label}: a pane changed its look while it moved, in ${seen.length} frame(s), e.g. ${seen[0]}`,
      )
  }
}

async function sidebarParks(page, failures, open) {
  const read = () =>
    page.evaluate((selector) => {
      const aside = document.querySelector(selector)
      if (!aside) return null
      return {
        hidden: aside.getAttribute("aria-hidden") === "true",
        inert: aside.hasAttribute("inert"),
        parked: aside.hasAttribute("data-parked"),
        skipped: getComputedStyle(aside).contentVisibility === "hidden",
        visible: getComputedStyle(aside).visibility === "visible",
      }
    }, css.sidebar)
  if (open) {
    const now = await read()
    if (!now || now.hidden || now.inert || now.parked || now.skipped || !now.visible)
      failures.push(`the opened sidebar is not reachable: ${JSON.stringify(now)}`)
    return
  }
  // Sliding (340 ms): hidden from assistive tech, focus already out of it,
  // but not parked yet — parking on the key restyled every row (#1177).
  let folding = await read()
  if (!(await waitFor(async () => (folding = await read())?.hidden === true, 1_000)))
    failures.push(
      `the folding sidebar is not hidden from assistive tech: ${JSON.stringify(folding)}`,
    )
  else if (folding.inert || folding.parked)
    failures.push(
      `the sidebar parked on the fold's key, not after its slide: ${JSON.stringify(folding)}`,
    )
  if (
    await page.evaluate(
      (selector) => document.querySelector(selector)?.contains(document.activeElement),
      css.sidebar,
    )
  )
    failures.push("focus stayed inside the folding sidebar")
  const parked = async () => {
    const now = await read()
    return Boolean(now?.inert && now.parked && now.skipped && !now.visible)
  }
  if (!(await waitFor(parked, 5_000)))
    failures.push(`the folded sidebar never parked: ${JSON.stringify(await read())}`)
}

/**
 * Choosing a channel shows its sessions: with the session list hidden, a
 * click on the main conversation's channel in the sidebar shows the list
 * again and leaves the one pane as it was. The person can hide it again.
 */
async function channelShowsList(page, failures) {
  const list = (state) => page.locator(`${css.workspace}[data-list="${state}"]`)
  await page.keyboard.press(keys.toggleSessionList)
  if (!(await waitFor(async () => (await list("closed").count()) === 1, 5_000))) {
    failures.push("the session list did not hide")
    return
  }
  await page
    .locator(`${css.sidebar} nav`)
    .getByText("Conversations", { exact: true })
    .first()
    .click({ timeout: 10_000 })
  if (!(await waitFor(async () => (await list("open").count()) === 1, 5_000)))
    failures.push("choosing a channel left the session list hidden")
  const panes = await page.locator(css.pane).count()
  if (panes !== 1) failures.push(`choosing a channel left ${panes} panes, not 1`)
}

/** The sidebar channel the main conversation is listed in. */
const MAIN_CHANNEL = "Conversations"

/**
 * A channel in the sidebar shows that something runs in it by its name
 * shimmering, with no glyph at its row's end, and its name says "Running" to
 * a reader. Its settings stay hidden until the row is pointed at. Without a
 * running turn the name is plain.
 */
async function channelRunning(page, name, running, failures) {
  const row = page
    .locator(`${css.sidebar} ${css.kitRowFrame}`)
    .filter({ has: page.getByText(name, { exact: true }) })
    .filter({ has: page.locator(".workspace-row-icon") })
    .first()
  const read = () =>
    row.evaluate((frame) => {
      // How much of an element shows: its opacity times every ancestor's.
      const shown = (element) => {
        if (!element) return null
        let opacity = 1
        for (let at = element; at; at = at.parentElement)
          opacity *= Number(getComputedStyle(at).opacity)
        return opacity
      }
      return {
        counted: frame.querySelector("[data-trailing='count']") !== null,
        shimmer: frame.querySelector(".workspace-shimmer[data-working]") !== null,
        spinner: frame.querySelectorAll('.workspace-status[data-status="running"]')
          .length,
        settings: shown(frame.querySelector("[data-channel-settings-open]")),
        said: (
          frame.querySelector("[data-size]")?.getAttribute("aria-label") ?? ""
        ).includes("Running"),
      }
    })
  if (!(await row.count())) {
    // The tree layout draws its channels as branches, whose controls already
    // show only under the pointer; this check is the columns layout's.
    const branch = page.locator(`${css.sidebar} [data-row="channel"]`, { hasText: name })
    if (!(await branch.count())) failures.push(`no channel ${name} in the sidebar`)
    return
  }
  // Away from the window's left edge, where the pointer would peek the sidebar out.
  await page.mouse.move(600, 400)
  const wanted = (now) =>
    now.spinner === 0 && now.shimmer === running && now.said === running
  let now = await read()
  await waitFor(async () => wanted((now = await read())), 10_000)
  if (now.spinner > 0)
    failures.push(
      `channel ${name} draws a turning arc (${now.spinner}): ${JSON.stringify(now)}`,
    )
  else if (!wanted(now))
    failures.push(
      `channel ${name}'s name ${running ? "does not shimmer while" : "still shimmers after"} a turn runs: ${JSON.stringify(now)}`,
    )
  if (now.settings === null) {
    failures.push(`channel ${name} has no settings`)
    return
  }
  // Settled, not after a fixed wait: the kit fades them over its transition.
  const atRest =
    (await waitFor(async () => {
      const at = await read()
      return at.settings === 0 ? at : null
    }, 5_000)) ?? (await read())
  if (atRest.settings !== 0)
    failures.push(`channel ${name} shows its settings at rest (${atRest.settings})`)
  await row.locator(":scope > [data-size]").hover({ timeout: 10_000 })
  const pointed =
    (await waitFor(async () => {
      const at = await read()
      return at.settings === 1 ? at : null
    }, 5_000)) ?? (await read())
  if (pointed.settings !== 1)
    failures.push(`channel ${name} pointed at shows its settings at ${pointed.settings}`)
  // The name keeps saying it works under the pointer: nothing takes its place.
  if (pointed.shimmer !== running)
    failures.push(
      `channel ${name} pointed at ${running ? "stops" : "starts"} shimmering: ${JSON.stringify(pointed)}`,
    )
  await page.mouse.move(600, 400)
}

/**
 * The pointer on a row's settings, beside its control, leaves the row its
 * hover fill: the frame owns hover, so moving onto the action does not drop
 * it. The fill at rest is read first; the hover fill is the one the control
 * moves to under the pointer. Each is read once it stops changing. Answers
 * whether there was a hover fill to compare.
 */
async function rowFillBesideItsSettings(page, row, name, failures) {
  const control = row.locator(":scope > [data-size]")
  const read = () =>
    control.evaluate((it) => ({
      fill: getComputedStyle(it).backgroundColor,
      hovered: Boolean(
        it.closest('[data-slot="sidebar-menu-item-row"]')?.matches(":hover"),
      ),
      animating: it
        .getAnimations()
        .some(
          (animation) =>
            ["running", "pending"].includes(animation.playState) &&
            animation.effect?.getComputedTiming().iterations !== Infinity,
        ),
    }))
  const settled = async () => {
    let last = null
    return (
      (await waitFor(async () => {
        const now = await read()
        if (now.animating) {
          last = null
          return null
        }
        const same = last !== null && now.fill === last.fill
        last = now
        return same ? now : null
      }, 5_000)) ??
      last ??
      (await read())
    )
  }
  await page.mouse.move(600, 400)
  const atRest = (await settled()).fill
  await control.hover({ timeout: 10_000 })
  // Waits past any transition delay for the fill to leave its rest.
  if (!(await waitFor(async () => (await read()).fill !== atRest, 2_000))) {
    // A selected row keeps one fill under the pointer and away from it, so
    // there is nothing to tell apart.
    log(
      `channel ${name} has the same fill at rest and pointed at, so its hover was not compared`,
    )
    return false
  }
  let onControl = await settled()
  // WebKit can drop :hover if a concurrent sidebar update shifts layout under
  // the stationary pointer; re-hover the control once before taking the baseline.
  if (!onControl.hovered || onControl.fill === atRest) {
    await control.hover({ timeout: 10_000 })
    await waitFor(async () => (await read()).fill !== atRest, 2_000)
    onControl = await settled()
  }
  await row.locator("[data-channel-settings-open]").hover({ timeout: 10_000 })
  const beside = await settled()
  if (beside.fill !== onControl.fill)
    failures.push(
      `channel ${name} pointed at its settings loses the row's hover fill: ${beside.fill}, not ${onControl.fill}`,
    )
  return true
}

/**
 * With two channels in the sidebar one is not selected, so its hover fill
 * differs from its rest and can be weighed beside its settings. The columns
 * layout's check: the tree draws channels as branches without these rows.
 */
async function channelRowsKeepHover(page, failures) {
  const rows = page
    .locator(`${css.sidebar} ${css.kitRowFrame}`)
    .filter({ has: page.locator("[data-channel-settings-open]") })
  const count = await rows.count()
  if (count < 2) {
    if (!(await page.locator(`${css.sidebar} [data-row="channel"]`).count()))
      failures.push(`the sidebar shows ${count} channel rows with settings, not 2`)
    return
  }
  let compared = false
  for (let index = 0; index < count && !compared; index++) {
    const row = rows.nth(index)
    const name = (await row.locator(":scope > [data-size]").textContent())?.trim() ?? ""
    compared = await rowFillBesideItsSettings(page, row, name, failures)
  }
  await page.mouse.move(600, 400)
  if (!compared)
    failures.push(`no channel row of ${count} has a hover fill apart from its rest fill`)
}

/**
 * While a turn runs, the window shows work one way: shimmering words, with no
 * turning arc in a pane's header, the sidebar, or the line under the turn.
 * A line under the turn, where one is drawn, is its words alone; a session
 * row in the sidebar's tree, where one lists the chat, shimmers its title.
 */
async function oneWorkingSign(run, conversationId, failures) {
  const read = () =>
    run.page.evaluate(
      ({ header, sidebar, id }) => {
        const arcs = (scope) =>
          [...document.querySelectorAll(scope)].reduce(
            (sum, at) =>
              sum +
              at.querySelectorAll('.workspace-status[data-status="running"]').length,
            0,
          )
        const lines = [...document.querySelectorAll(".workspace-live")]
        const row = document.querySelector(
          `${sidebar} .workspace-thread-row[data-session-row="${id}"]`,
        )
        return {
          header: arcs(header),
          sidebar: arcs(sidebar),
          lines: lines.length,
          lineArcs: arcs(".workspace-live"),
          plainLines: lines.filter((line) => !line.querySelector(".workspace-shimmer"))
            .length,
          row: row
            ? row.querySelector(".workspace-shimmer[data-working]") !== null
            : null,
        }
      },
      { header: css.paneHeader, sidebar: css.sidebar, id: conversationId },
    )
  // The tree's row learns of the turn from the list's next frame.
  const now =
    (await waitFor(async () => {
      const at = await read()
      return at.row !== false ? at : null
    }, 5_000)) ?? (await read())
  if (now.header > 0) failures.push(`a pane header draws a turning arc (${now.header})`)
  if (now.sidebar > 0) failures.push(`the sidebar draws a turning arc (${now.sidebar})`)
  if (now.lineArcs > 0)
    failures.push(`the line under the turn draws a turning arc (${now.lineArcs})`)
  if (now.plainLines > 0)
    failures.push(`${now.plainLines} line(s) under a running turn do not shimmer`)
  if (now.row === false)
    failures.push("the running chat's sidebar row does not shimmer its title")
  log(
    `one working sign: ${now.lines} live line(s), sidebar row ${now.row === null ? "not listed" : "shimmers"}`,
  )
  if (run.evidence)
    await run.page
      .screenshot({
        path: join(run.evidence, `working-${run.engine ?? "window"}.png`),
        timeout: 4_000,
        animations: "allow",
      })
      .catch((error) => log(`working screenshot: ${error.message.split("\n")[0]}`))
}

/** The compact rail and the window bar keep separate geometry. */
async function aroundTheLights(page, rail, failures) {
  const near = (a, b) => Math.abs(a - b) <= 0.5
  const holds = (now) =>
    now.rail === rail &&
    [now.at.workspace, now.at.inset, now.at.control].every(Number.isFinite) &&
    near(now.at.control, now.at.inset) &&
    now.at.control >= LIGHTS_END &&
    (rail === "open"
      ? near(now.at.workspace, now.at.railWidth) &&
        near(now.at.railTop, now.at.barBottom) &&
        near(now.at.railWidth, 64) &&
        near(now.at.item, now.at.railWidth / 2) &&
        // A narrow floating pill below the window bar, clear of the traffic
        // lights and the window's edges, holding the items and the toggle,
        // on the sidebar's material.
        now.at.pill !== null &&
        near(now.at.pill.width, 36) &&
        now.at.railFill === now.at.sidebarFill &&
        now.at.pill.top >= now.at.barBottom - 0.5 &&
        now.at.pill.left > 0 &&
        now.at.pill.bottom < now.at.windowBottom &&
        now.at.pill.radius >= now.at.pill.width / 2 - 0.5 &&
        near(now.at.pill.left + now.at.pill.width / 2, now.at.item) &&
        now.at.itemTop > now.at.pill.top &&
        // The corner row (the toggle and "nessa Studio") stands below it.
        // Down to the corner: no more than the toggle's foot room below it.
        now.at.windowBottom - now.at.pill.bottom <= 24 &&
        now.at.toggleBottom < now.at.pill.bottom &&
        // "nessa Studio" stands clear of the pill's edge.
        now.at.words >= now.at.pill.left + now.at.pill.width + 8 - 0.5
      : near(now.at.workspace, 0)) &&
    // The toggle is nessa's mark: in the footer's mark slot, right before
    // "nessa Studio", wherever the footer is drawn; at the corner otherwise.
    (rail === "off" ||
      (now.at.slot && now.at.toggleMiddle
        ? near(now.at.toggleMiddle[0], now.at.slot[0]) &&
          near(now.at.toggleMiddle[1], now.at.slot[1])
        : near(now.at.toggle, now.at.itemColumn)))
  if (await waitFor(async () => holds(await railLayout(page)), 10_000)) return true
  const now = await railLayout(page)
  failures.push(
    `rail ${rail} (now ${now.rail}) below window bar: ` + JSON.stringify(now.at),
  )
  return false
}

/**
 * The rail's state and, from the window's left edge: its width, the middle of
 * its first item and of its toggle, where the workspace and its first card start,
 * the window controls' inset from it, and where its titlebar's first control
 * starts.
 */
function railLayout(page) {
  return page.evaluate(
    ({ frame, workspace, control, rail, item, toggle, titlebar }) => {
      const box = (selector) => document.querySelector(selector)?.getBoundingClientRect()
      const middle = (selector) => {
        const at = box(selector)
        return at ? at.left + at.width / 2 : NaN
      }
      // A length token may be a calc(): resolved by laying it out.
      const length = (inside, token) => {
        const probe = document.createElement("div")
        probe.style.cssText = `position:absolute;visibility:hidden;width:var(${token})`
        inside.append(probe)
        const width = probe.getBoundingClientRect().width
        probe.remove()
        return width
      }
      // The workspace's first column: the rounded box at its left edge (the
      // docked sidebar, drawn flat but still cornered for its peek).
      const card = (inside) => {
        const left = inside.getBoundingClientRect().left
        const x = left + length(inside, "--desktop-gutter") + 2
        const panel = document
          .elementsFromPoint(x, innerHeight / 2)
          .find(
            (e) =>
              inside.contains(e) &&
              parseFloat(getComputedStyle(e).borderTopLeftRadius) >= 8 &&
              e.getBoundingClientRect().height > innerHeight / 2,
          )
        return panel?.getBoundingClientRect().left ?? NaN
      }
      const at = document.querySelector(workspace)
      return {
        rail: document.querySelector(frame)?.dataset.rail,
        at: {
          railWidth: box(rail)?.width ?? NaN,
          pill: (() => {
            const shape = document.querySelector(`${rail} .side-rail-pill`)
            const at = shape?.getBoundingClientRect()
            return at && at.width > 0
              ? {
                  top: at.top,
                  bottom: at.bottom,
                  left: at.left,
                  width: at.width,
                  radius: parseFloat(getComputedStyle(shape).borderTopLeftRadius),
                }
              : null
          })(),
          itemTop: box(item)?.top ?? NaN,
          // The fill each paints last (the sidebar's sits under its washes).
          railFill: (() => {
            const shape = document.querySelector(rail)
            return shape ? getComputedStyle(shape).backgroundColor : null
          })(),
          sidebarFill: (() => {
            const probe = document.createElement("div")
            probe.style.background = "var(--desktop-sidebar-material)"
            document.querySelector(frame)?.append(probe)
            const fill = getComputedStyle(probe).backgroundColor
            probe.remove()
            return fill
          })(),
          item: middle(item),
          toggle: box(toggle)?.left ?? NaN,
          toggleBottom: box(toggle)?.bottom ?? NaN,
          toggleTop: box(toggle)?.top ?? NaN,
          words: box(".side-rail-studio .desktop-identity-words")?.left ?? NaN,
          toggleMiddle: box(toggle)
            ? [
                box(toggle).left + box(toggle).width / 2,
                box(toggle).top + box(toggle).height / 2,
              ]
            : null,
          // The footer's mark slot the toggle stands in: the visible one, if any.
          slot: (() => {
            const found = [
              ...document.querySelectorAll(".workspace-identity .desktop-identity-mark"),
            ]
              .filter(
                (e) =>
                  getComputedStyle(e).visibility !== "hidden" && !e.closest("[inert]"),
              )
              .map((e) => e.getBoundingClientRect())
              .find((r) => r.width > 0 && r.left >= 0 && r.left < innerWidth)
            return found
              ? [found.left + found.width / 2, found.top + found.height / 2]
              : null
          })(),
          windowBottom: innerHeight,
          itemColumn: length(document.querySelector(frame), "--rail-column"),
          railTop: box(rail)
            ? box(rail).top +
              parseFloat(getComputedStyle(document.querySelector(rail)).paddingTop)
            : NaN,
          barBottom: box(titlebar)?.bottom ?? NaN,
          workspace: at.getBoundingClientRect().left,
          card: card(at),
          inset: length(document.querySelector(frame), "--desktop-window-controls-inset"),
          control: box(control)?.left ?? NaN,
        },
      }
    },
    {
      frame: css.workspaceWindow,
      workspace: css.workspace,
      control: `${css.titlebar} button`,
      titlebar: css.titlebar,
      rail: css.sideRail,
      item: `${css.sideRail} [data-rail-item]`,
      toggle: css.sideRailToggle,
    },
  )
}

/**
 * Toggles the rail from the page and follows "nessa Studio" every frame: it
 * stands in the corner beside the toggle, 8px past the toggle's mark with the
 * rail closed and 20px with it open (clear of the pill), moving only
 * between the two. Settled, the toggle (with the rail closed) stands on
 * Agents' icon column. The first pane's name, in the chat's titlebar, goes
 * only between where it stood and where it lands, never past either: in the
 * window's corner it stands still while its pane moves under it, rather than
 * ride the pane's flight from under the window's controls and back.
 * Returns what it found wrong.
 */
function footerThroughToggle(page) {
  // The engine as Playwright launched it: every context's user agent claims
  // Chrome on a Mac (`openPage`), so the page cannot tell WebKit by it.
  const webkit = page.context().browser()?.browserType().name() === "webkit"
  return page.evaluate(
    async ([toggleSelector, webkit]) => {
      const words = () =>
        document.querySelector(".side-rail-studio .desktop-identity-words")
      const name = () => document.querySelector(".split-panes-grid .workspace-pane-name")
      const names = []
      const toggle = document.querySelector(toggleSelector)
      const mark = () => toggle.querySelector("svg").getBoundingClientRect()
      const middle = (e) => {
        const at = e.getBoundingClientRect()
        return at.left + at.width / 2
      }
      const failures = []
      if (!words()) failures.push(`no "nessa Studio" beside the rail's toggle`)
      const seen = []
      // Where the name stood before the toggle: a flight's first frame already
      // draws the change.
      const from = name()?.getBoundingClientRect().left
      const started = performance.now()
      toggle.click()
      await new Promise((done) => {
        const frame = () => {
          if (words()) seen.push(words().getBoundingClientRect().left - mark().right)
          if (name()) names.push(name().getBoundingClientRect().left)
          if (performance.now() - started < 900) requestAnimationFrame(frame)
          else done()
        }
        requestAnimationFrame(frame)
      })
      const off = seen.find((gap) => gap < 7.5 || gap > 20.5)
      if (off !== undefined)
        failures.push(
          `"nessa Studio" stood ${off.toFixed(1)}px past the toggle's mark, not 8-20px`,
        )
      const want = toggle.getAttribute("aria-expanded") === "true" ? 20 : 8
      const last = seen.at(-1)
      if (last !== undefined && Math.abs(last - want) > 0.5)
        failures.push(
          `"nessa Studio" settled ${last.toFixed(1)}px past the toggle's mark, not ${want}px`,
        )
      if (from === undefined || !name())
        failures.push("no pane name in the chat's titlebar")
      const to = names.at(-1)
      // Chromium holds the name within half a pixel. WebKit leaves a few
      // (0.7, 1.3 and 2.5px seen in CI, never the same frame twice): the pane,
      // its counter-scaled bar and the name each move on their own transform,
      // and WebKit lands their sum a little off. Riding the pane's flight, as
      // before this check, moves the name 32px or more.
      const slack = webkit ? 4 : 1
      const strayed = names.find(
        (left) => left < Math.min(from, to) - slack || left > Math.max(from, to) + slack,
      )
      if (strayed !== undefined)
        failures.push(
          `the pane's name went to ${strayed.toFixed(1)}px, outside ${from.toFixed(1)}-${to.toFixed(1)}px, as the rail toggled (every frame: ${names.map((left) => left.toFixed(1)).join(" ")})`,
        )
      // Agents' icon is on screen only with the sidebar open.
      const icon = document.querySelector(
        '.workspace-window[data-sidebar="open"] .agents-overview-entry svg',
      )
      if (toggle.getAttribute("aria-expanded") === "false" && icon) {
        const apart = middle(toggle) - middle(icon)
        if (Math.abs(apart) > 0.5)
          failures.push(
            `the closed rail's toggle is ${apart.toFixed(1)}px off Agents' icon`,
          )
      }
      return failures
    },
    [css.sideRailToggle, webkit],
  )
}

/**
 * With the sidebar and the session list folded, the chat's pane stands alone
 * in the window's corner and its title stands past the window's controls.
 * Closing and opening the rail moves the pane under it; the title stays
 * where it is, every frame (`footerThroughToggle`), and the columns go back
 * as they were.
 */
async function titleWithThePanesAlone(page, failures) {
  const columns = () =>
    page.evaluate((workspace) => {
      const at = document.querySelector(workspace)
      return { sidebar: at?.dataset.sidebar, list: at?.dataset.list }
    }, css.workspace)
  const before = await columns()
  if (before.sidebar === "open") await page.keyboard.press(keys.toggleSidebar)
  if (before.list === "open") await page.keyboard.press(keys.toggleSessionList)
  const alone = await waitFor(
    () =>
      page.evaluate(
        (workspace) =>
          document.querySelector(workspace)?.hasAttribute("data-panes-alone"),
        css.workspace,
      ),
    5_000,
  )
  if (!alone) {
    failures.push(
      `folding the side columns left the panes not alone: ${JSON.stringify(await columns())}`,
    )
    return
  }
  // Let the folds land before the rail moves the pane again.
  await page.waitForTimeout(900)
  const name = (left) => `${left?.toFixed(1)}px`
  const where = () =>
    page.evaluate(
      () =>
        document
          .querySelector(".split-panes-grid .workspace-pane-name")
          ?.getBoundingClientRect().left,
    )
  const start = await where()
  for (const open of [false, true]) {
    failures.push(
      ...(await footerThroughToggle(page)).map((failure) => `panes alone: ${failure}`),
    )
    const now = await where()
    if (start === undefined || now === undefined || Math.abs(now - start) > 0.5)
      failures.push(
        `with the panes alone and the rail ${open ? "open" : "closed"}, the chat's title stands at ${name(now)}, not ${name(start)}`,
      )
  }
  if (before.sidebar === "open") await page.keyboard.press(keys.toggleSidebar)
  if (before.list === "open") await page.keyboard.press(keys.toggleSessionList)
  if (
    !(await waitFor(async () => {
      const now = await columns()
      return now.sidebar === before.sidebar && now.list === before.list
    }, 5_000))
  )
    failures.push(
      `the side columns did not open back: ${JSON.stringify(await columns())}`,
    )
}

/** The row of the conversation `conversationId` in the window's list. */
const sessionRow = (page, conversationId) =>
  page.locator(selectorFor.sessionListRowFor(conversationId))

/**
 * Chooses `label` in the More menu of `conversationId`'s row: the pointer
 * over the row shows its buttons, as a person sees them. The list re-sorts
 * as other conversations change, which can move the row out from under the
 * pointer and hide the button again; a person points at the row again, and
 * so does this, until the button takes the click.
 */
async function choose(page, conversationId, label) {
  const row = sessionRow(page, conversationId)
  const more = row.locator('[data-session-action="more"]')
  const end = Date.now() + 20_000
  for (;;) {
    await row.hover({ timeout: 15_000 })
    try {
      await more.click({ timeout: 2_000 })
      break
    } catch (error) {
      if (Date.now() > end) throw error
    }
  }
  await page
    .getByRole("menuitem", { name: label, exact: true })
    .click({ timeout: 10_000 })
}

/** Waits for `conversationId`'s row to say `title`. */
async function rowShown(page, conversationId, title, failures) {
  const shown = await sessionRow(page, conversationId)
    .filter({ has: page.getByText(title, { exact: true }) })
    .waitFor({ timeout: 15_000 })
    .then(() => true)
    .catch(() => false)
  if (!shown) failures.push(`the window never listed ${JSON.stringify(title)}`)
}

/**
 * Types `text` into the open conversation's composer and presses Send.
 * Resolves with the turns the conversation already had, so a turn after it
 * is one not among them. A read keeps only the latest turns, so a turn is
 * never found by its position. A send the window refuses or keeps is a
 * failure, not a retry. `known`, the turns the caller already read, skips
 * that read, which opens the conversation's agent. `where` is the composer,
 * when more than one pane shows one.
 */
async function send(
  run,
  conversationId,
  text,
  failures,
  known,
  where = dock(run.page),
  sendBy,
) {
  const before = known ?? (await seenTurns(run, conversationId))
  const box = where.locator("textarea")
  await box.fill(text)
  const button = where.getByRole(...composerControls.send)
  const timeout = sendBy === undefined ? 10_000 : sendBy - Date.now()
  if (timeout <= 0) throw new Error("the upgraded Send exhausted its click budget")
  await button.click({ timeout })
  const taken = await waitFor(async () => (await box.inputValue()) === "", 10_000)
  if (!taken) failures.push(`the composer kept ${JSON.stringify(text)} after Send`)
  run.sent.set(conversationId, (run.sent.get(conversationId) ?? 0) + 1)
  return before
}

/**
 * The `trace` turn's run while it is still at work, after its last call: the
 * run's trace alone says so. Open, it reads "Working" over a row per call,
 * and the call at work is named once, in the run, with no live row under it
 * saying the same; folded, its line still reads "Working". The run is live
 * only for the turn's hold, so what it draws is read in one go, and nothing
 * after that waits on it still being live.
 */
async function traceWhileLive(run, failures) {
  const drawn = await waitFor(
    () =>
      run.page.evaluate(
        (tools) => {
          const works = document.querySelectorAll(
            '.workspace-work[data-status="running"]',
          )
          if (works.length !== 1) return null
          const work = works[0]
          const rows = work.querySelectorAll('[data-slot="tool-trace-step"]')
          if (rows.length !== tools.length) return null
          const last = rows[rows.length - 1].textContent ?? ""
          const label = tools.find((tool) => last.includes(tool)) ?? null
          const count = (text) => (label ? (text ?? "").split(label).length - 1 : 0)
          // The turn's own message, and any live row drawn in its transcript:
          // an earlier run of the same calls is not this one.
          const live = [
            ...(work
              .closest(".workspace-transcript")
              ?.querySelectorAll(".workspace-live") ?? []),
          ]
          return {
            label,
            last,
            line: work.querySelector('[data-slot="tool-trace-line"]')?.textContent ?? "",
            rows: live.length,
            times:
              count(work.closest(".workspace-message")?.textContent) +
              live.reduce((sum, row) => sum + count(row.textContent), 0),
          }
        },
        TRACE_CALLS.map((call) => call.tool),
      ),
    15_000,
  )
  if (!drawn) {
    failures.push(
      `the window never drew the trace turn's ${TRACE_CALLS.length} calls as a live run`,
    )
    return
  }
  if (drawn.rows !== 0)
    failures.push(`a live row repeats the running trace under it (${drawn.rows})`)
  if (drawn.times !== 1)
    failures.push(
      `the call at work, ${drawn.label ?? JSON.stringify(drawn.last)}, is named ${drawn.times} times, not once`,
    )
  if (!drawn.line.includes("Working"))
    failures.push(`the live run's line reads ${JSON.stringify(drawn.line)}`)
  const work = run.page.locator('.workspace-work[data-status="running"]')
  const transcript = run.page.locator(".workspace-transcript").filter({ has: work })
  const line = work.locator('[data-slot="tool-trace-line"]')
  if (run.evidence) await liveShot(run, transcript, "live")
  // Folded while it works, its line is the one sign of it, still "Working".
  // A turn that ended first has nothing left to fold.
  try {
    await line.click({ timeout: 2_000 })
  } catch {
    if ((await work.count()) === 0) return
    throw new Error("the live run's line cannot be clicked")
  }
  const folded = await waitFor(
    async () =>
      (await work.count()) === 0 ||
      ((await line.getAttribute("aria-expanded", { timeout: 500 }).catch(() => null)) ===
        "false" &&
        ((await line.textContent({ timeout: 500 }).catch(() => "")) ?? "").includes(
          "Working",
        ) &&
        (await transcript.locator(".workspace-live").count()) === 0),
    2_000,
  )
  if (!folded) failures.push("the live run, folded, does not read Working on its own")
  else if (run.evidence) await liveShot(run, transcript, "live-folded")
}

/**
 * Chooses `label` ("Detailed" or "Compact") under Transcript in the main
 * conversation's pane menu, and waits for its transcript to say it is drawn
 * that way.
 */
async function chooseTranscriptView(run, label, failures) {
  const pane = run.page
    .locator(css.pane)
    .filter({ has: run.page.locator(css.conversationDock) })
  await pane.locator(css.paneActions).click({ timeout: 10_000 })
  const item = run.page.getByRole("menuitemradio", { name: label, exact: true })
  await item.waitFor({ timeout: 10_000 })
  if (run.evidence)
    await run.page
      .locator(css.menu)
      .screenshot({
        path: join(
          run.evidence,
          `transcript-${run.engine ?? "window"}-menu-${label}.png`,
        ),
        timeout: 4_000,
      })
      .catch((error) =>
        log(`transcript menu screenshot: ${error.message.split("\n")[0]}`),
      )
  await item.click({ timeout: 10_000 })
  const view = label.toLowerCase()
  const shown = await waitFor(
    async () =>
      (await pane
        .locator(".workspace-transcript")
        .getAttribute("data-transcript-view", { timeout: 1_000 })
        .catch(() => null)) === view,
    5_000,
  )
  if (!shown) failures.push(`choosing ${label} did not draw the transcript ${view}`)
  // The menu blinks its choice and fades after the view has changed; a click
  // on the trigger before it has gone closes the next menu as it opens.
  await run.page.locator(css.menu).waitFor({ state: "detached", timeout: 5_000 })
  return pane
}

/**
 * Evidence of the live run, taken from the page by the transcript's box: an
 * element's own screenshot waits for it to keep still, which a shimmering
 * line never does within the hold. A run that ended first is not shot.
 */
async function liveShot(run, transcript, name) {
  const box = await transcript.boundingBox({ timeout: 1_000 }).catch(() => null)
  if (!box) return
  await run.page
    .screenshot({
      path: join(run.evidence, `trace-${run.engine ?? "window"}-${name}.png`),
      clip: box,
      timeout: 4_000,
    })
    .catch((error) => log(`trace ${name} screenshot: ${error.message.split("\n")[0]}`))
}

/** The execution ids of the turns `conversationId` has now. */
async function seenTurns(run, conversationId) {
  const view = await run.client.conversation.read(conversationId)
  return new Set(view.messages.map((each) => each.executionId))
}

/**
 * The newest turn the gateway sent again on behalf of the one sent as `text`:
 * a message held behind a failure is stopped with it and sent again. The
 * agent is given a notice before the text; the view keeps the person's own
 * words and marks the turn `retried`.
 */
function carried(view, seen, text) {
  return view.messages.findLast(
    (each) =>
      !seen.has(each.executionId) && each.retried === true && each.userText === text,
  )
}

/**
 * The turn sent as `text`, once it ends, checked: `text` is in the
 * conversation exactly once, and its turn ended `status`. A turn stopped
 * behind a failure is followed to the turn that carries it on. `seen` are the
 * turns from before the send, left out of what a failure lists.
 */
/**
 * Whether the gateway was built with terminals; a failure naming how to
 * build it when not (`scripts/terminal/ghostty.sh`).
 */
/**
 * ⌘\ offers terminals: the open one by its name, under the terminal glyph,
 * chats under their agent's tile, and a new terminal, which opens beside.
 * The new one is ended from its own pane, leaving `opened`'s in front.
 */
async function opensBeside(run, opened, name, listed, failures) {
  const page = run.page
  await page
    .locator(`.workspace-terminal-pane${css.focusedPane} .workspace-pane-title`)
    .click()
  const panes = await page.locator(css.pane).count()
  await page.keyboard.press(keys.openBeside)
  const dialog = page.getByRole("dialog", { name: "Open beside" })
  if (
    !(await dialog.waitFor({ timeout: 5_000 }).then(
      () => true,
      () => false,
    ))
  ) {
    failures.push("⌘\\ beside a terminal opened no Open beside picker")
    return
  }
  const terminalRow = dialog.getByRole("option", { name: new RegExp(`^${name}`) })
  if (!(await terminalRow.count()))
    failures.push(`the Open beside picker does not offer the terminal "${name}"`)
  else if (!(await terminalRow.locator("svg").count()))
    failures.push("the Open beside picker's terminal row has no icon")
  if (!(await dialog.locator('[role="option"] .workspace-agent-tile').count()))
    failures.push("the Open beside picker shows no agent tile on its chats")
  const fresh = dialog.getByRole("option", { name: /^New terminal in #/ })
  if (!(await fresh.count())) {
    failures.push("the Open beside picker offers no New terminal")
    await page.keyboard.press(keys.escape)
    return
  }
  await fresh.click()
  const made = await waitFor(async () => {
    const now = await run.client.terminals.list()
    return now.find(
      (each) =>
        each.kind === "shell" &&
        each.terminalId !== opened.terminalId &&
        !listed.some((old) => old.terminalId === each.terminalId),
    )
  }, 10_000)
  if (!made) {
    failures.push("New terminal in the Open beside picker opened no terminal")
    return
  }
  if (
    !(await waitFor(
      async () => (await page.locator(css.pane).count()) === panes + 1,
      5_000,
    ))
  )
    failures.push("New terminal in the Open beside picker did not open a pane beside")
  const beside = page.locator(`.workspace-terminal-pane${css.focusedPane}`)
  if ((await beside.locator(".workspace-pane-title").innerText()) === name)
    failures.push("the new terminal did not take the focus beside")
  await beside.getByRole("button", { name: "End Terminal" }).click()
  if (
    !(await waitFor(async () => (await page.locator(css.pane).count()) === panes, 5_000))
  )
    failures.push("ending the terminal opened beside left its pane")
}

/**
 * A watched Run that ended stays open, its box no taller than what it
 * printed; the bar's chevron folds the output away and brings it back.
 */
async function foldsItsOutput(block, failures, shot) {
  const output = block.locator("[data-run-output]")
  const fold = block.locator("[data-run-fold]")
  if (!(await output.count())) {
    failures.push("a Run that ended while watched folded its output away")
    return
  }
  const fitted = await waitFor(async () => {
    const [box, grid] = await Promise.all([
      output.boundingBox(),
      output.locator('[data-slot="terminal-grid"]').boundingBox(),
    ])
    return box && grid && box.height < grid.height
  }, 5_000)
  if (!fitted) failures.push("an ended Run's box keeps the blank rows under its output")
  await shot?.("open")
  await fold.click()
  if (
    !(await waitFor(async () => (await output.count()) === 0, 3_000)) ||
    (await fold.getAttribute("aria-expanded")) !== "false"
  )
    failures.push("the Run bar's chevron did not fold the output away")
  await shot?.("folded")
  await fold.click()
  if (!(await waitFor(async () => (await output.count()) === 1, 3_000)))
    failures.push("the Run bar's chevron did not bring the output back")
}

async function terminalsBuilt(run, failures) {
  try {
    await run.client.terminals.list()
    return true
  } catch (error) {
    failures.push(
      `the gateway has no terminals (${error.code ?? error.message}; it logged ${JSON.stringify(terminalLog(run))}): build it with eval "$(scripts/terminal/ghostty.sh)" && cargo build -p nessa-server --features nessa-server/terminals`,
    )
    return false
  }
}

/** What the gateway logged about terminals, last few lines: why one is unavailable. */
function terminalLog(run) {
  return run.stack.gateway
    .log()
    .split("\n")
    .filter((line) => /terminal|pty/i.test(line))
    .slice(-4)
}

/**
 * Whether the terminal in `grid` reflows to its pane: with the window made
 * narrower, the grid draws at full size (no scale) and holds fewer columns,
 * which only a resize of the terminal itself gives. A pane that never took
 * the terminal's size scales it down instead.
 */
async function reflowsToItsPane(page, grid, failures) {
  const look = () =>
    grid.evaluate((element) => ({
      width: element.getBoundingClientRect().width,
      scaled: getComputedStyle(element).transform !== "none",
    }))
  const settled = await waitFor(
    async () => (await look()).width > 0 && !(await look()).scaled,
    5_000,
  )
  if (!settled) {
    failures.push("the terminal opened scaled down: its pane never took its size")
    return
  }
  const before = await look()
  const size = page.viewportSize()
  if (!size) return
  await page.setViewportSize({ width: size.width - 320, height: size.height })
  try {
    const reflowed = await waitFor(async () => {
      const now = await look()
      return !now.scaled && now.width < before.width - 100 ? now : null
    }, 5_000)
    if (!reflowed)
      failures.push(
        `a narrower window did not reflow the terminal (${JSON.stringify({ before, after: await look() })})`,
      )
  } finally {
    await page.setViewportSize(size)
  }
}

/**
 * Opens a second shell with ⌘J (it takes the focused pane), then clicks the
 * first terminal's row: the pane shows the first terminal again, `word` and
 * all, with no click inside it. The second shell is ended through the
 * gateway after. Whether the pane came back.
 */
async function switchesBack(run, grid, row, listed, opened, word) {
  const page = run.page
  await page.keyboard.press(keys.newTerminal)
  const second = await waitFor(async () => {
    const now = await run.client.terminals.list()
    return now.find(
      (each) =>
        each.kind === "shell" &&
        each.terminalId !== opened.terminalId &&
        !listed.some((old) => old.terminalId === each.terminalId),
    )
  }, 10_000)
  const away = await waitFor(
    async () => !(await gridRows(grid)).some((each) => each.trim() === word),
    10_000,
  )
  await row.click()
  const back = await waitFor(
    async () => (await gridRows(grid)).some((each) => each.trim() === word),
    10_000,
  )
  if (second) await run.client.terminals.close(second.terminalId).catch(() => {})
  return Boolean(second && away && back)
}

/** A terminal grid's rows as text, top to bottom. */
async function gridRows(grid) {
  return grid
    .locator('[data-slot="terminal-row"]')
    .evaluateAll((rows) => rows.map((row) => row.textContent ?? ""))
    .catch(() => [])
}

/** Shows the main conversation in the focused pane again, after a step put a terminal there. */
async function showMainAgain(run, failures) {
  const row = run.page.locator(selectorFor.sessionListRowFor(run.main.id))
  await row.click({ timeout: 10_000 }).catch(() => {})
  const shown = await dock(run.page)
    .locator("textarea")
    .waitFor({ timeout: 10_000 })
    .then(
      () => true,
      () => false,
    )
  if (!shown) failures.push("the main conversation did not show again after the terminal")
}

async function settled(run, conversationId, seen, text, status, failures, seconds = 60) {
  const end = Date.now() + seconds * 1000
  let view
  let turn
  do {
    try {
      view = await readWithReviews(run.client, conversationId)
      turn = view.messages.find((each) => each.userText === text)
      if (turn?.status === "cancelled" && status !== "cancelled")
        turn = carried(view, seen, text) ?? turn
      if (turn && ended(turn) && !(turn.status === "cancelled" && status !== "cancelled"))
        break
    } catch (error) {
      log(`read ${conversationId}: ${error.message}`)
    }
    await sleep(200)
  } while (Date.now() < end)
  if (!turn) {
    failures.push(
      `${JSON.stringify(text)} is not in the conversation after ${seconds} s: ${JSON.stringify(
        view?.messages
          .filter((each) => !seen.has(each.executionId))
          .map((each) => [each.status, each.userText.slice(0, 60)]),
      )}`,
    )
    return { view }
  }
  const copies = view.messages.filter((each) => each.userText === text).length
  if (copies !== 1) failures.push(`${JSON.stringify(text)} was sent ${copies} times`)
  if (!ended(turn)) failures.push(`${JSON.stringify(text)} is still ${turn.status}`)
  else if (turn.status !== status)
    failures.push(
      `${JSON.stringify(text)} ended ${turn.status}${turn.error ? ` (${turn.error})` : ""}, not ${status}`,
    )
  return { view, turn }
}

async function expectShown(page, text, failures, ms) {
  if (!(await shows(page, text, ms)))
    failures.push(`the window does not show ${JSON.stringify(text)}`)
}

/**
 * How long a review the deadline would deny waits, as the gateway arms it
 * when `agents.reviewWaitSeconds` is left out (`REVIEW_WAIT` in
 * crates/nessa-server/src/composition/agent.rs). The stress gateway leaves it
 * out, so this is the default a person gets.
 */
const REVIEW_WAIT_MS = 300_000

/** The visible agent card, once there is one; `null` after `ms`. */
async function card(page, ms = 30_000) {
  const cards = page.locator(css.agentApproval)
  const seen = await cards
    .first()
    .waitFor({ timeout: ms })
    .then(() => true)
    .catch(() => false)
  return seen ? cards : null
}

/** Answers the one card shown, by the effect `answer` names, from the review's own options. */
async function answerCard(
  run,
  conversationId,
  answer,
  failures,
  reason,
  { click, review } = {},
) {
  const cards = await card(run.page)
  if (!cards) {
    failures.push(
      `no approval card${reason ? ` for ${JSON.stringify(reason)}` : ""} within 30 s`,
    )
    return false
  }
  // The card just answered can still be leaving: the one to answer is the
  // card of the review with that title. A card names its review by key
  // (`data-review`), since an agent's card does not repeat its title.
  const { view } = await untilTurn(
    run,
    conversationId,
    0,
    (_turn, current) =>
      current.permissions.some(
        (each) =>
          (!reason || each.title === reason) && (!review || each.review === review),
      ),
    10,
  )
  const permission = view?.permissions?.find(
    (each) => (!reason || each.title === reason) && (!review || each.review === review),
  )
  const asking = permission
    ? cards.and(run.page.locator(`[data-review="${permission.review}"]`))
    : cards
  const showsIt = await asking
    .first()
    .waitFor({ timeout: 30_000 })
    .then(() => true)
    .catch(() => false)
  if (!showsIt) {
    failures.push(`no card for the review ${JSON.stringify(reason ?? review)}`)
    return false
  }
  if (!(await waitFor(async () => (await cards.count()) === 1, 5_000)))
    failures.push(`${await cards.count()} approval cards shown, not 1`)
  // A review the deadline would deny waits long enough to answer from the
  // phone, and the card counts it down in minutes.
  const deadline = permission?.deadline
  if (deadline?.effect === "deny") {
    if (deadline.waitMs !== REVIEW_WAIT_MS)
      failures.push(
        `a review waits ${deadline.waitMs} ms before it is denied, not ${REVIEW_WAIT_MS}`,
      )
    const said = await asking
      .first()
      .locator('[role="timer"]')
      .getAttribute("aria-label", { timeout: 5_000 })
      .catch(() => null)
    if (!said || !/within \d+ minutes?,/.test(said))
      failures.push(`the card's countdown says ${JSON.stringify(said)}, not minutes`)
  }
  const label = offeredLabel(permission?.options, answer === "allow" ? "allow" : "deny")
  if (!label) {
    failures.push(`the review offers no ${answer}`)
    return false
  }
  const option = permission.options.find((each) => each.effect === answer)
  const observation = run.reviewAnswers.begin({
    conversationId,
    executionId: permission.executionId,
    review: permission.review,
    permissionId: permission.permissionId,
    optionId: option.id,
  })
  let acknowledged = false
  let gone
  try {
    if (click) await click(label)
    else
      await asking
        .first()
        .getByRole("button", { name: label, exact: true })
        .click({ timeout: 10_000 })
    ;[acknowledged, gone] = await Promise.all([
      waitFor(() => observation.acknowledged(), 30_000),
      waitFor(async () => {
        const current = await readWithReviews(run.client, conversationId)
        return !current.permissions.some((each) => each.review === permission.review)
      }, 30_000),
    ])
  } finally {
    const evidence = observation.finish()
    acknowledged = reviewAnswerAcknowledged(evidence, acknowledged)
    run.reviewAnswerEvidence.push(evidence)
    if (run.evidence)
      writeView(
        run.evidence,
        `views/${run.engine}-review-answers.json`,
        run.reviewAnswerEvidence,
      )
    if (!acknowledged)
      failures.push(
        `the answer to ${JSON.stringify(reason ?? permission.title)} was ${evidence.outcome}`,
      )
  }
  if (!gone)
    failures.push(
      `the answered review ${JSON.stringify(reason ?? permission.title)} is still listed`,
    )
  return acknowledged && gone
}

/** Picks `mode` in the open conversation's access shield. */
async function pickAccess(page, mode) {
  await dock(page).locator(composerControls.access).click({ timeout: 10_000 })
  await page.locator(composerControls.accessMenu).waitFor({ timeout: 10_000 })
  await page
    .locator(composerControls.accessMenu)
    .getByRole(...composerControls.accessItem[mode])
    .click({ timeout: 10_000 })
  // Menu teardown restores focus asynchronously. Finish that interaction
  // before the next fill focuses a field and inserts its text separately.
  const restored = await waitFor(
    () =>
      dock(page)
        .locator(composerControls.access)
        .evaluate((node) => node === document.activeElement),
    10_000,
  )
  if (!restored) throw new Error("the access menu did not restore focus to its control")
}

/**
 * The session's own permission mode a scenario turn says it ran at
 * (`SAYS.ranAt`), fixed when the turn began; undefined when it said none.
 */
function ranAt(turn) {
  const said = (turn?.parts ?? [])
    .filter((part) => part.kind === "text")
    .map((part) => part.text)
    .join("")
  const at = said.lastIndexOf(SAYS.ranAt)
  return at < 0 ? undefined : said.slice(at + SAYS.ranAt.length).trim()
}

/**
 * The red card's whole round, once the chat's agent offers {@link ACCESS}:
 * the agent asks, the card says Nessa's words, Approve raises the access and
 * only then answers, and the card lists what was allowed.
 */
async function approveAccess(run, step, conversationId, named, failures) {
  const title = await titleOf(run, conversationId)
  await openStressConversation(run.page, conversationId, title)
  const asking = prompt(run, STRESS.requestAccess, step)
  await run.client.conversation.submit(
    run.client.conversation.prepare(conversationId, asking),
  )
  const { view: asked, turn: askingTurn } = await settled(
    run,
    conversationId,
    new Set(named?.messages.map((each) => each.executionId) ?? []),
    asking,
    "completed",
    failures,
  )
  if (failures.length) return
  const rowAsk = async () => {
    const { conversations } = await run.client.conversation.list({})
    return conversations.find((each) => each.conversationId === conversationId)?.ask
  }
  const ask = await waitFor(rowAsk, 15_000)
  if (!ask?.access) {
    failures.push(`the list row carries ${JSON.stringify(ask)}, not an access request`)
    return
  }
  if (
    ask.access.mode !== ACCESS.access ||
    ask.access.grants.length !== 3 ||
    ask.question !== "Allow auto approval in this chat?" ||
    ask.choices.map((each) => each.label).join("|") !== `${STRESS.approved}|Not now` ||
    ask.recommended !== 1 ||
    ask.reason !== ACCESS.reason
  )
    failures.push(`the row's access request is ${JSON.stringify(ask)}`)
  const card = run.page.locator(`[data-access-request="${ACCESS.access}"]`)
  if (
    !(await card
      .waitFor({ timeout: 15_000 })
      .then(() => true)
      .catch(() => false))
  ) {
    failures.push("the chat does not draw the red access card")
    return
  }
  if (!(await card.getByText(ask.question).count()))
    failures.push("the access card does not say Nessa's question")
  if (!(await card.getByText(ACCESS.reason).count()))
    failures.push("the access card does not say the agent's reason")
  if (await card.locator("[data-slot=access-request-grants]").count())
    failures.push("the access card lists what is allowed before it is approved")
  if ((await run.client.conversation.read(conversationId)).approvalMode !== "ask")
    failures.push("asking for access changed the chat's access")
  await card.getByRole("button", { name: STRESS.approved, exact: true }).click()
  if (!(await accessIs(run, conversationId, ACCESS.access)))
    failures.push("Approve did not raise the chat to the access asked for")
  const { turn: answered } = await settled(
    run,
    conversationId,
    new Set(asked?.messages.map((each) => each.executionId) ?? []),
    STRESS.approved,
    "completed",
    failures,
  )
  // The agent's session says the permission mode each turn began at: the
  // Approve turn must begin at the raised one, not the Ask the asking turn
  // ran at. That is fixed when the answer went, whenever it is read.
  const before = ranAt(askingTurn)
  const after = ranAt(answered)
  if (!before || !after)
    failures.push(
      `the scenario did not say the mode its turns ran at (${before}, ${after})`,
    )
  else if (after === before)
    failures.push(
      `the Approve turn ran at ${after}, the mode the chat asked at: the answer went before the access changed`,
    )
  await expectShown(run.page, `${SAYS.approvedAccess}${STRESS.approved}`, failures)
  const listed = card.locator("[data-slot=access-request-grants] li")
  const lines = await waitFor(async () => {
    const shown = await listed.allTextContents()
    return shown.length === 3 ? shown : null
  }, 15_000)
  if (!lines || lines.join("|") !== ask.access.grants.join("|"))
    failures.push(
      `the approved card lists ${JSON.stringify(lines)}, not ${JSON.stringify(ask.access.grants)}; it says ${JSON.stringify(await card.textContent().catch(() => null))}`,
    )
  if (await card.getByRole("button").count())
    failures.push("the approved card still offers a choice")
  if (
    await waitFor(async () => (await rowAsk()) === undefined, 15_000).then(
      (cleared) => !cleared,
    )
  )
    failures.push("the list row still carries the approved access request")
}

/** Waits for the gateway to say the conversation runs at `mode`. */
const accessIs = (run, conversationId, mode, ms = 20_000) =>
  waitFor(async () => {
    try {
      return (await run.client.conversation.read(conversationId)).approvalMode === mode
    } catch {
      return false
    }
  }, ms)

/** Waits until Send has turned into Stop. */
const stopShown = (page, ms = 15_000) =>
  dock(page)
    .getByRole(...composerControls.stop)
    .waitFor({ timeout: ms })
    .then(() => true)
    .catch(() => false)

const sendShown = (page, ms = 15_000) =>
  dock(page)
    .getByRole(...composerControls.send)
    .waitFor({ timeout: ms })
    .then(() => true)
    .catch(() => false)

/** A text no other turn of the run has: the kind's phrase, the step and the seed. */
const prompt = (run, phrase, step, extra = "") =>
  `${phrase} r${step.index}${extra} s${run.knobs.seed}`

/**
 * One of the `fresh` step's other conversations: created and sent to on
 * `client`, then followed on it to its end. Registered for the rest check
 * before it is created, so one whose create took effect and lost its answer
 * is still swept.
 */
async function startedBeside(run, client, step, n, failures) {
  const conversationId = randomUUID()
  const first = prompt(run, STRESS.reply, step, ` burst ${n}`)
  try {
    await client.conversation.create({ conversationId, agent: run.stack.agent })
  } catch (error) {
    failures.push(
      `a conversation started beside the window's was refused: ${explain(error)}`,
    )
    return
  }
  // Once it exists the rest check sweeps it, even if the submit's answer is
  // lost while its turn runs.
  run.checked.push(conversationId)
  try {
    await client.conversation.submit(client.conversation.prepare(conversationId, first))
  } catch (error) {
    failures.push(
      `a conversation started beside the window's was refused: ${explain(error)}`,
    )
    return
  }
  await settled(
    { ...run, client },
    conversationId,
    new Set(),
    first,
    "completed",
    failures,
  )
}

/**
 * Points the selected agent's command and its absolute file arguments at a
 * new directory. A script becomes a wrapper there that runs the real file:
 * a symlink would make `process.argv[1]` differ from the path the script
 * records, so it would never start, and a copy would break a root it
 * derives from its own path. The node binary and every other file are
 * symlinks, so the bytes stay where they are.
 */
function moveStagedRuntime(directory) {
  const configPath = join(directory, "ci", "instances", INSTANCE, "config.json")
  const config = JSON.parse(readFileSync(configPath, "utf8"))
  const runtime = config.agents.runtimes[config.agents.selected]
  const staged = mkdtempSync(join(tmpdir(), "nessa-stress-runtime-"))
  let n = 0
  const wrapScript = (path) => {
    const dest = join(staged, `${n++}-${basename(path)}`)
    writeFileSync(
      dest,
      `import { spawnSync } from "node:child_process"
import { realpathSync } from "node:fs"
const result = spawnSync(realpathSync(process.execPath), [${JSON.stringify(path)}, ...process.argv.slice(2)], { stdio: "inherit" })
process.exit(result.status ?? 1)
`,
    )
    return dest
  }
  const relocate = (path) => {
    if (typeof path !== "string" || !isAbsolute(path) || !existsSync(path)) return path
    if (lstatSync(path).isDirectory()) return path
    if (/\.(?:mjs|cjs|js)$/.test(path)) return wrapScript(path)
    const dest = join(staged, `${n++}-${basename(path)}`)
    symlinkSync(path, dest)
    return dest
  }
  runtime.command = relocate(runtime.command)
  // Only the staged runtime moves. An absolute argument such as
  // `--config /profiles/staging.toml` is the restoration identity.
  runtime.stagedRuntime = (runtime.stagedRuntime ?? []).map(relocate)
  writeFileSync(configPath, JSON.stringify(config))
  chmodSync(configPath, 0o600)
}

/**
 * Whether `turn`, the first after a handoff, was the one given the earlier
 * turns, on `model`: the scenario answers that turn alone with `SAYS.carried`
 * and the prompt it was given, which quotes `earlier`, a message said before.
 */
function carriedBy(turn, model, what, earlier, failures) {
  const said = (turn?.parts ?? [])
    .filter((part) => part.kind === "text")
    .map((part) => part.text)
    .join("")
  if (!said.startsWith(`${SAYS.carried}${model}: `))
    failures.push(
      `the first turn after ${what} said ${JSON.stringify(said.slice(0, 200))}, not that it carried the earlier turns on ${model}`,
    )
  else if (!said.includes(JSON.stringify(earlier)))
    failures.push(`the first turn after ${what} was not given ${JSON.stringify(earlier)}`)
}

/**
 * Whether the window shows, whole, the reply on `model` to `message`: it says
 * back the prompt it answered, which ends with `message`, so no other reply
 * quotes it, and it is shown whole once that end is.
 */
async function carriedShown(page, model, message) {
  return page
    .locator('.workspace-message[data-role="agent"]:visible')
    .filter({ hasText: `${SAYS.carried}${model}` })
    .filter({ hasText: message })
    .first()
    .waitFor({ timeout: 15_000 })
    .then(() => true)
    .catch(() => false)
}

/** The scripted agent a `handoff` step switches to: the one the run does not start on. */
function otherAgent(agent) {
  return agent === "claude" ? "codex" : "claude"
}

/** The `picker` step's work, with the stars cleared around it by the step. */
async function pickerStep(run, step, failures) {
  const page = run.page
  const chip = dock(page).locator(css.modelChip)
  if (!(await waitFor(async () => !(await chip.isDisabled()), 10_000))) {
    failures.push("the established conversation's model chip is disabled")
    return
  }
  const before = await readWithReviews(run.client, run.main.id)
  const current = catalog.models.find((model) => model.modelId === before.runtime?.model)
  const bars = (model) =>
    (model.reasoning?.effortLevels ?? []).filter((level) => level !== "none")
  const target =
    current &&
    catalog.models.find(
      (model) =>
        model.provider === current.provider &&
        model.modelId !== current.modelId &&
        bars(model).length >= 2,
    )
  if (!target)
    throw new CannotRun(
      "no second catalog model with levels in this conversation's provider",
    )
  // The chat goes to Auto where its model offers it. A target not offered at
  // Auto on this platform (only Ask is verified for every model) takes the
  // chat to Ask in the same change, and the shield says so; one offered at
  // Auto keeps it.
  const { agents } = await run.client.agents.list()
  const offered = (modelId) =>
    agents
      .flatMap((agent) => agent.models)
      .find((model) => model.modelId === modelId)
      ?.approvalModes?.map((choice) => choice.id) ?? ["ask"]
  const startMode = offered(current.modelId).includes("auto") ? "auto" : "ask"
  if (before.approvalMode !== startMode) {
    await pickAccess(page, startMode)
    if (!(await accessIs(run, run.main.id, startMode))) {
      failures.push(`the chat did not come to ${startMode} before the model change`)
      return
    }
  }
  const endMode = offered(target.modelId).includes(startMode) ? startMode : "ask"
  const row = () =>
    page.locator(css.modelOption).filter({
      has: page
        .locator(css.modelOptionLabel)
        .getByText(target.displayName, { exact: true }),
    })
  const opened = () => page.locator(css.modelPicker).waitFor({ timeout: 5_000 })
  const closed = () =>
    page
      .locator(css.modelPicker)
      .waitFor({ state: "detached", timeout: 5_000 })
      .then(() => true)
      .catch(() => false)
  const star = async () => {
    const button = row().locator(css.modelStar)
    await row().hover()
    await button.click()
    if ((await button.getAttribute("aria-pressed")) !== "true")
      failures.push(`${target.displayName}'s star is not on once pressed`)
  }

  // A star, then the picker opens on Starred with the model listed once.
  await chip.click()
  await opened()
  await page.locator(css.modelSearch).fill(target.displayName)
  await star()
  await page.locator(css.modelSearch).fill("")
  await page.keyboard.press(keys.escape)
  if (!(await closed())) failures.push("Escape did not close the model picker")
  await chip.click()
  await opened()
  const onStarred = await page
    .locator(`${css.modelRail}[data-filter="starred"]`)
    .getAttribute("aria-current")
  if (onStarred !== "true")
    failures.push("with a star, the picker did not open on Starred")
  if ((await row().count()) !== 1)
    failures.push(`Starred does not list ${target.displayName} once`)
  if (run.evidence)
    await page
      .locator(css.modelPicker)
      .screenshot({ path: join(run.evidence, `picker-${run.engine}-starred.png`) })
      .catch(() => {})

  if ((await row().getAttribute("aria-disabled")) === "true")
    throw new CannotRun(`${target.displayName} cannot be chosen in this conversation`)
  // A sideways swipe over the row moves its level one step, and the picker
  // stays open. Fingers moving right (a negative deltaX) light one bar more.
  const lit = () => row().locator(`${css.modelBar}[data-lit]`).count()
  const total = await row().locator(css.modelBar).count()
  const litBefore = await lit()
  const up = litBefore < total
  await row().hover()
  // What the page received, for the failure message: each wheel's sideways
  // distance, unit, and time.
  await page.evaluate(() => {
    const seen = []
    window.__pickerWheels = seen
    window.addEventListener(
      "wheel",
      (event) =>
        seen.push([
          event.deltaX,
          event.deltaY,
          event.deltaMode,
          Math.round(event.timeStamp),
        ]),
      { capture: true, passive: true },
    )
  })
  // A swipe is a run of small moves less than 160 ms apart (the kit's
  // gesture gap); a pause longer than that starts a new swipe. Checking the
  // bars between moves takes longer than that, so one probe move learns how
  // far a move reaches in this engine, and after a pause that ends it, one
  // burst carries just past one step (44 px).
  const sign = up ? -1 : 1
  await page.mouse.wheel(sign * 16, 0)
  const reach = Math.abs(
    await page.evaluate(() => window.__pickerWheels.at(-1)?.[0] ?? 0),
  )
  await page.waitForTimeout(400)
  const moves = reach > 0 ? Math.floor(44 / reach) + 1 : 0
  await Promise.all(Array.from({ length: moves }, () => page.mouse.wheel(sign * 16, 0)))
  const litAfter = litBefore + (up ? 1 : -1)
  if (!(await waitFor(async () => (await lit()) === litAfter, 5_000)))
    failures.push(
      `a sideways swipe left ${await lit()} of ${target.displayName}'s bars lit, not ${litAfter} (wheels seen [dx, dy, mode, ms]: ${await page.evaluate(() => JSON.stringify(window.__pickerWheels))})`,
    )
  if ((await page.locator(css.modelPicker).count()) === 0)
    failures.push("a sideways swipe closed the model picker")

  // One press of a bar: that model at that level, and the picker closes.
  const bar = row().locator(css.modelBar).nth(1)
  const level = (await bar.getAttribute("aria-label"))?.split(", ").at(-1)
  await bar.click()
  if (!(await closed())) failures.push("a bar left the model picker open")
  const named = `Change model, currently ${target.displayName}, ${level}`
  if (
    !(await waitFor(async () => (await chip.getAttribute("aria-label")) === named, 5_000))
  )
    failures.push(
      `a bar left the chip "${await chip.getAttribute("aria-label")}", not "${named}"`,
    )
  const fast = dock(page).locator(css.composerFast)
  if ((await fast.isDisabled()) === target.fastMode)
    failures.push(
      `Fast is${target.fastMode ? " not" : ""} offered for ${target.displayName}`,
    )

  // The next message runs on it, and the chip still names that level.
  const text = prompt(run, STRESS.model, step)
  const seen = await send(run, run.main.id, text, failures)
  const confirmed = await waitFor(async () => {
    const view = await readWithReviews(run.client, run.main.id)
    return view.runtime?.model === target.modelId && !view.modelChange ? view : null
  }, 15_000)
  if (!confirmed) failures.push(`sending did not run on ${target.displayName}`)
  await settled(run, run.main.id, seen, text, "completed", failures)
  await expectShown(page, `${SAYS.model}${target.modelId}`, failures)
  const access = (await readWithReviews(run.client, run.main.id)).approvalMode
  if (access !== endMode)
    failures.push(
      `after the change to ${target.displayName} the chat runs at ${access}, not ${endMode}`,
    )
  const stepped =
    endMode !== startMode
      ? `Access is now Ask for approval. This model doesn’t offer ${startMode === "auto" ? "Auto approval" : "Full access"} here.`
      : undefined
  // The shield says it, as its tooltip; the composer has no line for it.
  const noticeText = () =>
    dock(page)
      .locator(composerControls.access)
      .getAttribute("data-tooltip")
      .then((text) => text ?? "")
      .catch(() => "")
  if (stepped && !(await waitFor(async () => (await noticeText()) === stepped, 10_000)))
    failures.push(`the shield did not say the chat moved to Ask: "${await noticeText()}"`)
  if (!stepped && (await noticeText()).includes("Access is now"))
    failures.push(
      `the shield says access moved though it was kept: "${await noticeText()}"`,
    )
  await composerSaysNothing(page, "after a model change", failures)
  if (
    (await dock(page)
      .locator(composerControls.access)
      .getAttribute("data-access-mode")) !== composerControls.accessValue[endMode]
  )
    failures.push(`the shield does not show ${endMode} after the model change`)
  if (run.evidence && stepped)
    await dock(page)
      .screenshot({ path: join(run.evidence, `picker-${run.engine}-stepped.png`) })
      .catch(() => {})
  if ((await chip.getAttribute("aria-label")) !== named)
    failures.push(
      `after the send the chip says "${await chip.getAttribute("aria-label")}"`,
    )

  // The star off again, so later runs open where the model is.
  await chip.click()
  await opened()
  // On Starred, the model leaves the list as its star comes off.
  await row().hover()
  await row().locator(css.modelStar).click()
  if (
    !(await row()
      .waitFor({ state: "detached", timeout: 5_000 })
      .then(() => true)
      .catch(() => false))
  )
    failures.push(`unstarred, ${target.displayName} stayed under Starred`)
  await page.keyboard.press(keys.escape)
  if (!(await closed())) failures.push("Escape did not close the model picker")
  if (run.evidence)
    writeFileSync(
      join(run.evidence, `picker-${run.engine ?? "window"}-${step.index}.json`),
      JSON.stringify(
        {
          before: current.modelId,
          after: target.modelId,
          level,
          access: { before: startMode, after: endMode },
          confirmed: confirmed?.runtime,
        },
        null,
        2,
      ),
    )
}

/** Picks `model` in a composer's or a new channel's picker, by its exact name. */
async function pickModel(page, picker, model) {
  await selectStressModel(page, picker, model.displayName)
}

/**
 * Makes a channel with the sidebar's + as a person does, answering each
 * question as it is asked, and answers the gateway's listing of it; `null`
 * when it was not made. With `refusedFolder`, that folder is chosen first and
 * Create must be refused at the folder question, before a folder made for it
 * is taken instead. With `dropped`, those files and folders are dragged onto
 * the window as the host tells it (`dropOnChannel`).
 */
async function makeChannel(
  run,
  failures,
  { name, folder, refusedFolder, notes, resource, dropped, model, coordinator = true },
) {
  const { page } = run
  const pick = (kind, value) =>
    page.evaluate(
      ([kind, value]) => window.__fakeHostPicks[kind].push(value),
      [kind, value],
    )
  const asking = (stepName) =>
    page
      .locator(`${css.newChannelAsked}[data-step="${stepName}"]`)
      .waitFor({ timeout: 10_000 })
      .then(() => true)
      .catch(() => {
        failures.push(`the new channel flow never asked its ${stepName}`)
        return false
      })
  // With evidence asked for, each question is photographed as it is asked.
  const photograph = (label) =>
    run.evidence
      ? page
          .screenshot({ path: join(run.evidence, `channel-${run.engine}-${label}.png`) })
          .catch(() => {})
      : Promise.resolve()
  const head = page.locator(`${css.sidebar} .workspace-section-head`).first()
  await head.hover()
  await head.locator(css.channelAdd).click({ timeout: 10_000 })
  const flow = page.locator(css.newChannel)
  if (!(await asking("name"))) return null
  // One title and no instructions under it; the field sits right under the
  // question it answers, not at the foot of an empty page.
  if ((await flow.locator(".new-channel-head p").count()) !== 0)
    failures.push("the New channel title still has a line of instructions under it")
  const gap = await page.evaluate((flow) => {
    const asked = document.querySelector(`${flow} .new-channel-asked`)
    const composer = document.querySelector(`${flow} .new-channel-composer-row`)
    return asked && composer
      ? composer.getBoundingClientRect().top - asked.getBoundingClientRect().bottom
      : null
  }, css.newChannel)
  if (gap === null || gap > 64)
    failures.push(
      `the New channel field is ${gap}px below its question, not right under it`,
    )
  await page.locator(`${css.newChannelField}[data-step="name"]`).fill(name)
  await photograph(`${name}-1-name`)
  await page.keyboard.press(keys.enter)
  if (!(await asking("folder"))) return null
  const folderChoice = refusedFolder ?? folder
  if (folderChoice) {
    await pick("folders", folderChoice)
    await flow.getByRole("button", { name: "Choose a folder…" }).click()
    await flow
      .getByRole("button", { name: "Use this folder" })
      .waitFor({ timeout: 5_000 })
    await photograph(`${name}-2-folder`)
    await flow.getByRole("button", { name: "Use this folder" }).click()
  } else {
    await flow.getByRole("button", { name: "Make one for me" }).click()
  }
  if (!(await asking("placement"))) return null
  // Both choices must remain independent, and Local must not allocate a VM.
  const coordinatorLocation = flow.getByLabel("Main agent location")
  const workerLocation = flow.getByLabel("New threads location")
  // Boat is offered only while it is connected. A run without a connected
  // Boat (no keys, or a gateway that cannot say) skips these choices rather
  // than failing on Boat's setup.
  const boat = await boatProbe(run.client)
  const note = flow.locator("[data-new-channel-placement-note]")
  const boatConnect = flow.locator('[data-cloud-connect="boat"]')
  // All local, the step says nothing about clouds.
  if ((await note.count()) || (await flow.locator("[data-cloud-connect]").count()))
    failures.push("with everything local, the placement step still talks about clouds")
  if (!boat.ready) {
    // A cloud not connected is still offered; chosen, it asks to be connected
    // right here, and the question waits until it is or another place is.
    await workerLocation.selectOption("boat")
    if (!(await boatConnect.isVisible()))
      failures.push("New threads on a Boat not connected does not offer to connect it")
    if (!(await note.isVisible()))
      failures.push(
        "a cloud is chosen, yet the placement step does not say what that means",
      )
    else await saidWhole(page, "[data-new-channel-placement-note]", "placement", failures)
    await photograph(`${name}-3-placement-connect`)
    await flow.locator(".new-channel-placement").focus()
    await page.keyboard.press(keys.enter)
    if (
      !(await flow.locator(`${css.newChannelAsked}[data-step="placement"]`).isVisible())
    )
      failures.push("the placement question moved on while its Boat was not connected")
    await workerLocation.selectOption("local")
    if (await boatConnect.count())
      failures.push("Boat's connect form stays after the threads went back to Local")
  } else {
    const offered = await waitFor(
      () =>
        coordinatorLocation
          .locator('option[value="boat"]')
          .evaluate((option) => !option.disabled),
      5_000,
    )
    if (!offered)
      failures.push("Boat is connected, yet the placement step does not offer it")
    else {
      await coordinatorLocation.selectOption("boat")
      if ((await workerLocation.inputValue()) !== "local")
        failures.push("changing coordinator placement also changed worker placement")
      await coordinatorLocation.selectOption("local")
      await workerLocation.selectOption("boat")
      if ((await coordinatorLocation.inputValue()) !== "local")
        failures.push("changing worker placement also changed coordinator placement")
      if (await boatConnect.count())
        failures.push("Boat is connected, yet choosing it asks to connect it")
      if (!(await note.isVisible()))
        failures.push(
          "a cloud is chosen, yet the placement step does not say what that means",
        )
      else
        await saidWhole(page, "[data-new-channel-placement-note]", "placement", failures)
      await workerLocation.selectOption("local")
    }
  }
  // On unless asked off; either way it can be switched later in settings.
  const coordinatorChoice = flow.locator("[data-new-channel-coordinator]")
  if ((await coordinatorChoice.inputValue()) !== "on")
    failures.push("a new channel's coordinator is not on until it is turned off")
  if (!coordinator) await coordinatorChoice.selectOption("off")
  await photograph(`${name}-3-placement`)
  await flow.locator(".new-channel-placement").focus()
  await page.keyboard.press(keys.enter)
  if (!(await asking("model"))) return null
  await photograph(`${name}-3-model`)
  if (model) {
    await pickModel(page, flow.locator(css.modelChip), model)
    await flow.getByRole("button", { name: "Use this model", exact: true }).click()
  } else await page.keyboard.press(keys.enter)
  if (!(await asking("notes"))) return null
  await saidWhole(page, `${css.newChannelField}[data-step="notes"]`, "notes", failures)
  if (notes) await page.locator(`${css.newChannelField}[data-step="notes"]`).fill(notes)
  await photograph(`${name}-4-notes`)
  await page.keyboard.press(keys.enter)
  if (!(await asking("resources"))) return null
  if (resource) {
    await pick("resources", [resource])
    await flow.getByRole("button", { name: "Add files…" }).click()
    const row = flow.locator(css.newChannelResource, { hasText: basename(resource) })
    if (
      !(await row.waitFor({ timeout: 5_000 }).then(
        () => true,
        () => false,
      ))
    )
      failures.push(`the attached ${basename(resource)} is not listed in the flow`)
  }
  if (dropped) await dropOnChannel(run, failures, dropped, photograph, name)
  await photograph(`${name}-5-resources`)
  const create = flow.getByRole("button", { name: /^Create channel/ })
  await create.click()
  if (refusedFolder) {
    if (!(await asking("folder"))) return null
    const said = await flow
      .locator(`${css.newChannelAsked}[data-step="folder"]`)
      .getByText("Agents can’t work there", { exact: false })
      .waitFor({ timeout: 10_000 })
      .then(
        () => true,
        () => false,
      )
    if (!said)
      failures.push(`choosing ${refusedFolder} was not refused at the folder question`)
    await photograph(`${name}-6-refused`)
    await flow.getByRole("button", { name: "Make one for me" }).click()
    await create.click()
  }
  const gone = await flow.waitFor({ state: "detached", timeout: 15_000 }).then(
    () => true,
    () => false,
  )
  if (!gone) {
    failures.push(
      `the channel ${name} was not made: ${fold(await flow.innerText().catch(() => ""))}`,
    )
    await page.keyboard.press(keys.escape)
    return null
  }
  const { channels } = await run.client.channels.list()
  const listed = channels.find((channel) => channel.name === name)
  if (!listed) failures.push(`the gateway does not list the channel ${name}`)
  if (listed && listed.coordinatorOn !== coordinator)
    failures.push(
      `the channel ${name} was made with its coordinator ${listed.coordinatorOn ? "on" : "off"}`,
    )
  if (
    listed &&
    (listed.coordinatorPlacement !== "local" ||
      listed.threadPlacement !== "local" ||
      !listed.coordinatorConversationId)
  )
    failures.push(
      `the channel ${name} did not retain its Local placements and main identity`,
    )
  return listed ?? null
}

/** The desktop window's least width, as `tauri.conf.json` sets it. */
const NARROWEST_WINDOW = 800

/**
 * The composer's hint for a question (a span's text, or an empty field's
 * placeholder) fits its box on one line, whole: it is neither wrapped nor
 * cut short. Measured as the text itself, in the box's own font, so a hint
 * that is clipped to one line still fails.
 */
async function saidWhole(page, selector, question, failures) {
  // At the narrowest the desktop window goes (`minWidth` in tauri.conf.json),
  // where the composer has the least room.
  const size = page.viewportSize()
  if (size) await page.setViewportSize({ width: NARROWEST_WINDOW, height: size.height })
  const said = await page.locator(selector).evaluate((node) => {
    const text =
      "placeholder" in node && node.value === "" ? node.placeholder : node.textContent
    const style = getComputedStyle(node)
    const context = document.createElement("canvas").getContext("2d")
    context.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`
    const room =
      node.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)
    return { text, wide: context.measureText(text).width, room }
  })
  if (size) await page.setViewportSize(size)
  if (said.wide > said.room + 1)
    failures.push(
      `the ${question} question's hint "${said.text}" needs ${Math.ceil(said.wide)}px on one line; it has ${Math.floor(said.room)}px`,
    )
}

/**
 * Drags `dropped` (each `{ path, kind }`) onto the window while the flow asks
 * for files, as the host tells the page: a drag over it lights the whole flow
 * as the drop target, and the drop lists each path with its kind's icon, once,
 * after anything already attached, and puts the target out.
 */
async function dropOnChannel(run, failures, dropped, photograph, name) {
  const { page } = run
  const flow = page.locator(css.newChannel)
  const heard = await waitFor(
    () => page.evaluate(() => window.__fakeHostListening("nessa://paths-dropped") > 0),
    5_000,
  )
  if (!heard) {
    failures.push("the new channel flow does not listen for dropped files")
    return
  }
  // The hint says it whole, on one line, and the count is short.
  await saidWhole(page, `${css.newChannel} .new-channel-hint`, "files", failures)
  const count = (await flow.locator(".new-channel-count").innerText()).trim()
  if (!/^\d\/\d$/.test(count))
    failures.push(`the flow counts its questions as "${count}"`)
  const emit = (event, payload) =>
    page.evaluate(
      ([event, payload]) => window.__fakeHostEmit(event, payload),
      [event, payload],
    )
  await emit("nessa://paths-dragging", { dragging: true })
  const target = page.locator(`${css.newChannel}[data-dropping] ${css.newChannelDrop}`)
  if (
    !(await target.waitFor({ timeout: 5_000 }).then(
      () => true,
      () => false,
    ))
  ) {
    failures.push("a drag over the new channel flow shows no drop target")
  } else {
    await photograph(`${name}-5-drop-target`)
  }
  const before = await flow.locator(css.newChannelResource).count()
  // The host says the drag is gone as it lets go and names the drop, then,
  // once it has looked at them, which paths that drop held.
  await emit("nessa://paths-dragging", { dragging: false })
  await emit("nessa://paths-drop-began", { drop: `stress-${name}` })
  // Until the paths come, Create waits: the draft is not yet what was dropped.
  const create = flow.getByRole("button", { name: /^Create channel/ })
  const held = await waitFor(() => create.isDisabled(), 2_000)
  if (!held) failures.push("Create can be pressed while dropped files are still coming")
  await emit("nessa://paths-dropped", {
    drop: `stress-${name}`,
    paths: [...dropped, dropped[0]],
    unread: 0,
  })
  const listed = await waitFor(
    async () =>
      (await flow.locator(css.newChannelResource).count()) === before + dropped.length,
    5_000,
  )
  if (!listed)
    failures.push(
      `dropping ${dropped.length} paths listed ${(await flow.locator(css.newChannelResource).count()) - before}`,
    )
  for (const { path, kind } of dropped) {
    const row = flow.locator(css.newChannelResource, { hasText: basename(path) })
    const listedAs = await row
      .getAttribute("data-kind", { timeout: 2_000 })
      .catch(() => null)
    if (listedAs !== kind)
      failures.push(
        `the dropped ${basename(path)} is listed as ${listedAs ?? "nothing"}, not ${kind}`,
      )
  }
  // Out, and the questions back at full ink once their fade settles.
  const out = await waitFor(
    () =>
      page.evaluate(
        ([flow, drop]) =>
          document.querySelector(drop) === null &&
          getComputedStyle(document.querySelector(`${flow} .new-channel-scroll`))
            .opacity === "1",
        [css.newChannel, css.newChannelDrop],
      ),
    2_000,
  )
  if (!out) failures.push("the drop target stays lit after the drop")
}

/** Forks `sourceId`, a conversation in channel `made`, from its row's More
 * menu, and checks the fork is listed in that channel. */
async function forkInChannel(run, made, sourceId, failures) {
  const listed = new Set(
    (await run.client.conversation.list({})).conversations.map(
      (each) => each.conversationId,
    ),
  )
  await choose(run.page, sourceId, "Fork")
  const fork = await waitFor(async () => {
    const { conversations } = await run.client.conversation.list({})
    return conversations.find((each) => !listed.has(each.conversationId))
  }, 15_000)
  if (!fork) {
    failures.push(`the gateway never listed a fork of a chat in ${made.name}`)
    return
  }
  run.checked.push(fork.conversationId)
  if (fork.channelId !== made.channelId)
    failures.push(
      `a fork of a chat in ${made.name} is in ${fork.channelId ?? "no channel"}`,
    )
}

/**
 * The `await` turn in `parentId`: the agent makes a thread naming where it
 * runs (`here`), sends to it and waits for its reply with one subscribe
 * call. Answers the thread's id, or `undefined` when the turn failed.
 */
async function awaitThread(run, step, parentId, failures, extra = "") {
  const text = prompt(run, STRESS.await, step, extra)
  const seen = await send(run, parentId, text, failures)
  const { turn } = await settled(run, parentId, seen, text, "completed", failures)
  if (!turn) return undefined
  const said = (turn.parts ?? []).map((part) => part.text ?? "").join("")
  const found = new RegExp(`${SAYS.awaited}([0-9a-f-]+): (.*)`, "s").exec(said)
  if (!found) {
    failures.push(`the await turn did not say what it waited for: ${said.slice(0, 160)}`)
    return undefined
  }
  const agentReply = `${SAYS.echo}Agent-authored input`
  if (
    !found[2].startsWith(agentReply) ||
    !found[2].includes(`${STRESS.reply} awaited by the parent`)
  )
    failures.push(
      `one subscribe call did not return the thread's reply: ${found[2].slice(0, 160)}`,
    )
  const expected = fold(`${SAYS.awaited}${found[1]}: ${found[2]}`)
  const shown = await waitFor(async () => {
    const lines = await run.page
      .locator(`${css.focusedPane} .workspace-message[data-role="agent"] p:visible`)
      .evaluateAll(chatIdentityTexts)
    return lines.some((line) => fold(line) === expected)
  }, 30_000)
  if (!shown)
    failures.push(
      `the window does not show the exact awaited chat identity and reply: ${expected}`,
    )
  // The reply came back in the call, so no report wakes the conversation.
  // A report would be queued as soon as the thread's turn settled, which
  // was before the call returned.
  await sleep(3_000)
  const view = await run.client.conversation.read(parentId)
  if (
    view.messages.some(
      (each) =>
        !seen.has(each.executionId) &&
        each.userText.startsWith(STRESS.report) &&
        each.userText.includes(found[1]),
    )
  )
    failures.push("a report followed the reply the agent already took with subscribe")
  return found[1]
}

/**
 * A chat started from the window in a channel whose threads use the pool,
 * while a worker serves it. Its agent runs on that worker, so it is told the
 * channel's name and notes, and neither the channel's folder nor the files
 * attached to it: both are this gateway's paths, which it could not open.
 * Checked at rest here, while the worker serves, as in `pool`.
 */
async function pooledChannelChat(run, step, failures) {
  const name = `stress-pooled-r${step.index}-s${run.knobs.seed}`
  const notes = `Notes for ${name}.`
  const attached = mkdtempSync(join(run.stack.scratch, "pooled-"))
  writeFileSync(join(attached, "brief.md"), "# Brief\n")
  const resource = realpathSync(join(attached, "brief.md"))
  const channel = await run.client.channels.create({
    requestId: name,
    name,
    agent: run.stack.agent,
    model: run.stack.profile.model,
    notes,
    resources: [resource],
    threadPlacement: "pool",
  })
  const text = prompt(run, STRESS.channel, step, ` ${name}`)
  try {
    const conversationId = await run.exclusive(async () => {
      // The sidebar lists the channel once the window reads the index again;
      // a vault sealed by the linking restart asks to be unlocked first.
      await run.page.reload()
      await unlockRequestedVault(run.client, run.page)
      await run.page
        .locator(`${css.sidebar} nav`)
        .getByText(name, { exact: true })
        .first()
        .click({ timeout: 15_000 })
      await run.page.keyboard.press(keys.newSession)
      const home = run.page.locator(`${css.focusedPane} ${css.paneHome}`)
      await home.locator(css.field).waitFor({ timeout: 15_000 })
      await home.locator(css.field).fill(text)
      await home.getByRole(...composerControls.send).click({ timeout: 10_000 })
      return waitFor(() => selectedStressConversation(run.page, text), 30_000)
    })
    if (!conversationId) {
      failures.push(`the chat started in ${name} was never listed`)
      return
    }
    const { view } = await settled(
      run,
      conversationId,
      new Set(),
      text,
      "completed",
      failures,
    )
    if (view?.lease?.environment !== "peer")
      failures.push(
        `the chat in ${name} runs on ${view?.lease?.environment}, not a worker`,
      )
    const reply = (view?.messages.find((each) => each.userText === text)?.parts ?? [])
      .filter((part) => part.kind === "text")
      .map((part) => part.text)
      .join("")
    if (!reply.startsWith(SAYS.channel))
      failures.push(
        `the agent in ${name} did not say where it works: ${fold(reply.slice(0, 300))}`,
      )
    // Every agent says where it works, which must not be the channel's
    // folder here; only Claude's reply also says its system prompt, so only
    // there can the notes and the attached file be checked.
    const told = run.stack.agent === "claude"
    for (const path of told ? [channel.folder, resource] : [channel.folder])
      if (reply.includes(path))
        failures.push(`the agent on a worker was told this gateway's path ${path}`)
    if (told && !reply.includes(notes))
      failures.push(`the agent in ${name} was not told the channel's notes`)
    failures.push(...(await atRest(run, [conversationId])))
  } finally {
    // Its threads go back here, as in `place`: once the workers stop, a new
    // thread on the pool would have nowhere to run.
    await run.client.channels
      .setPlacement({
        requestId: `${name}-back`,
        channelId: channel.channelId,
        coordinatorPlacement: "local",
        threadPlacement: "local",
      })
      .catch((error) => failures.push(`${name} did not go back here: ${explain(error)}`))
    // Back to the default channel: the chat above left its list showing.
    await run.page
      .locator(`${css.sidebar} nav`)
      .getByText("Conversations", { exact: true })
      .first()
      .click({ timeout: 10_000 })
      .catch(() => {})
  }
}

/**
 * The first message of a new session in `channel`, from the home Create left
 * open: it is listed in that channel, and its agent works in the channel's
 * folder, told the channel's notes (Claude's system prompt, where it has one).
 * Answers the session's id, or `undefined` when it was never listed.
 */
async function talkInChannel(run, step, channel, failures, notes, attached = []) {
  const text = prompt(run, STRESS.channel, step, ` ${channel.name}`)
  const home = run.page.locator(`${css.focusedPane} ${css.paneHome}`)
  await home.locator(css.field).waitFor({ timeout: 15_000 })
  await home.locator(css.field).fill(text)
  await home.getByRole(...composerControls.send).click({ timeout: 10_000 })
  const conversationId = await waitFor(
    () => selectedStressConversation(run.page, text),
    30_000,
  )
  if (!conversationId) {
    failures.push(`the first message in ${channel.name} was never listed`)
    return undefined
  }
  run.checked.push(conversationId)
  const { view } = await settled(
    run,
    conversationId,
    new Set(),
    text,
    "completed",
    failures,
  )
  const { conversations } = await run.client.conversation.list({})
  const summary = conversations.find((each) => each.conversationId === conversationId)
  if (summary?.channelId !== channel.channelId)
    failures.push(
      `the conversation started in ${channel.name} is in ${summary?.channelId ?? "no channel"}`,
    )
  const reply = (view?.messages.find((each) => each.userText === text)?.parts ?? [])
    .filter((part) => part.kind === "text")
    .map((part) => part.text)
    .join("")
  if (!reply.includes(`${SAYS.channel}${channel.folder}\n`))
    failures.push(
      `the agent in ${channel.name} did not work in ${channel.folder}: ${fold(reply.slice(0, 300))}`,
    )
  if (notes && run.stack.agent === "claude" && !reply.includes(notes))
    failures.push(`the agent in ${channel.name} was not told the channel's notes`)
  for (const path of run.stack.agent === "claude" ? attached : [])
    if (!reply.includes(path))
      failures.push(`the agent in ${channel.name} was not told of ${path}`)
  // A local channel's chat runs on this computer, the usual place, so its
  // heading says nothing under the title: no start time, place or lease
  // (UI audit #6). The pane menu and the shield say those.
  const heading = run.page.locator(`${css.focusedPane} .workspace-heading`)
  const quiet = await waitFor(
    async () =>
      (await heading.count()) === 1 &&
      (await heading.locator("p").count()) === 0 &&
      (await heading.locator("[data-workspace-place]").count()) === 0,
    10_000,
  )
  if (!quiet)
    failures.push(
      `the chat in ${channel.name} says more than its title: ${(await heading.count()) ? await heading.innerText() : "no heading"}`,
    )
  return conversationId
}

/**
 * The focused chat's right panel, as `read_window` must say it: two widgets
 * opened there, the other tab brought to the front, the panel hidden and
 * shown, then the other chat focused. `conversationId` is the chat in front
 * when this starts, and it is again when this returns.
 */
async function focusedChatPanel(run, conversationId, step, failures) {
  const page = run.page
  const text = prompt(run, STRESS.panelTabs, step)
  const seen = await send(
    run,
    conversationId,
    text,
    failures,
    undefined,
    page.locator(css.focusedPane).locator(css.conversationDock),
  )
  const { turn } = await settled(run, conversationId, seen, text, "completed", failures)
  if (!turn) return
  const talk = await inlineApp(page, '[data-talk="panel"]')
  const pin = await inlineApp(page, '[data-pin="panel"]')
  if (!talk || !pin) {
    failures.push(
      `the focused chat drew ${talk ? "" : "no talk board"}${talk || pin ? "" : " and "}${pin ? "" : "no panel pin"}`,
    )
    return
  }
  await talk.waitForSelector('body[data-talk-state="live"]', { timeout: 20_000 })
  // The button is in the app's frame. Activating it is the click the person
  // makes; a pointer click from the page does not reach it once two apps
  // share the pane.
  await talk.evaluate(() => document.querySelector('[data-talk="panel"]').click())
  const tabs = () => page.locator(`${css.chatPanelTabs} [role="tab"]`)
  if (!(await waitFor(async () => (await tabs().count()) === 1, 10_000))) {
    failures.push("the talk board did not open a panel tab")
    return
  }
  // The front tab is the only panel frame. Opening the pin replaces it, and
  // a sandbox document still loading is aborted (proxy.html, net::ERR_ABORTED).
  if (!(await panelAppReady(page, 'body[data-talk-state="live"]'))) {
    failures.push(
      "the talk board's panel frame was still loading when the next tab opened",
    )
    return
  }
  await pin.waitForSelector('body[data-pin-state="live"]', { timeout: 20_000 })
  await pin.evaluate(() => document.querySelector('[data-pin="panel"]').click())
  if (!(await waitFor(async () => (await tabs().count()) === 2, 10_000))) {
    failures.push("the pin did not open a second panel tab")
    return
  }
  // Opening a tab replaces the inline frame. Its resource fetch abort is the
  // one the mount already took, the same as the huddle step.
  noteLiveMountResourceAbort(run.opened, "live", run.stack.gateway.url)
  const plugins = ["mcp:mcptest", "mcp:panelpin"]
  const owned = (panel) =>
    panel?.tabs?.length === 2 &&
    panel.tabs.every((tab) => tabIdBelongs(tab, conversationId)) &&
    plugins.every((plugin) => panel.tabs.some((tab) => tab.plugin === plugin))
  const opened = await windowRead(
    run,
    conversationId,
    step,
    " panel open",
    failures,
    (said) => {
      const panel = said.windows?.[0]?.panel
      return (
        panel?.open === "open" && owned(panel) && panel.front?.plugin === "mcp:panelpin"
      )
    },
  )
  if (!opened) return
  const openPanel = opened.windows[0].panel
  if (!(await panelAppReady(page, 'body[data-pin-state="live"]'))) {
    failures.push("the pin's panel frame was still loading when its tab left the front")
    return
  }
  const other = page.locator(`${css.chatPanelTabs} [role="tab"][aria-selected="false"]`)
  await other.click()
  if (!(await panelAppReady(page, 'body[data-talk-state="live"]'))) {
    failures.push("the talk board's panel frame was still loading when the chat changed")
    return
  }
  const front = await windowRead(
    run,
    conversationId,
    step,
    " panel front",
    failures,
    (said) => {
      const panel = said.windows?.[0]?.panel
      return (
        panel?.open === "open" &&
        samePanelTabs(panel, openPanel) &&
        panel.front?.plugin === "mcp:mcptest" &&
        panel.front?.id !== openPanel.front.id
      )
    },
  )
  if (!front) return
  const shown = front.windows[0].panel
  await page.keyboard.press("Meta+Alt+KeyB")
  if (
    !(await waitFor(
      async () =>
        (await page.locator("#workspace-panel").getAttribute("aria-hidden")) === "true",
      3_000,
    ))
  ) {
    failures.push("⌥⌘B did not hide the panel")
    return
  }
  const hidden = await windowRead(
    run,
    conversationId,
    step,
    " panel hidden",
    failures,
    (said) => {
      const panel = said.windows?.[0]?.panel
      return (
        panel?.open === "closed" &&
        samePanelTabs(panel, shown) &&
        panel.front?.id === shown.front.id
      )
    },
  )
  if (!hidden) return
  await page.keyboard.press("Meta+Alt+KeyB")
  if (
    !(await waitFor(
      async () =>
        (await page.locator("#workspace-panel").getAttribute("aria-hidden")) === null,
      3_000,
    ))
  ) {
    failures.push("⌥⌘B did not show the panel")
    return
  }
  const focusedKey = () => page.locator(css.focusedPane).getAttribute("data-pane-key")
  const from = await focusedKey()
  await page.locator(`${css.pane}:not([data-pane-focused]) textarea`).click()
  if (!(await waitFor(async () => (await focusedKey()) !== from, 5_000))) {
    failures.push("clicking the other pane did not bring its chat to the front")
    return
  }
  const elsewhere = await windowRead(
    run,
    run.main.id,
    step,
    " panel other chat",
    failures,
    (said) => {
      const panel = said.windows?.[0]?.panel
      return (
        said.you?.threadId === run.main.id &&
        panel?.open === "open" &&
        panel.tabs?.length === 0 &&
        panel.front === undefined
      )
    },
  )
  if (!elsewhere) return
  const backFrom = await focusedKey()
  await page.locator(`${css.pane}:not([data-pane-focused]) textarea`).click()
  if (!(await waitFor(async () => (await focusedKey()) !== backFrom, 5_000))) {
    failures.push("clicking the first chat did not bring it back to the front")
    return
  }
  // Coming back draws the front tab again. Leave only after its document
  // has loaded, so a later chat does not abort that sandbox request.
  if (!(await panelAppReady(page, 'body[data-talk-state="live"]'))) {
    failures.push("the talk board's panel frame was still loading when the step left it")
    return
  }
  await windowRead(run, conversationId, step, " panel back", failures, (said) => {
    const panel = said.windows?.[0]?.panel
    return (
      panel?.open === "open" &&
      samePanelTabs(panel, shown) &&
      panel.front?.id === shown.front.id
    )
  })
  // Leave the panel closed, as the step found it: a later window read measures
  // the panes against the window's edge.
  await page.keyboard.press("Meta+Alt+KeyB")
  if (
    !(await waitFor(
      async () =>
        (await page.locator("#workspace-panel").getAttribute("aria-hidden")) === "true",
      3_000,
    ))
  )
    failures.push("⌥⌘B did not hide the panel again")
}

function tabIdBelongs(tab, conversationId) {
  try {
    const id = JSON.parse(tab.id)
    return Array.isArray(id) && id[0] === conversationId
  } catch {
    return false
  }
}

function samePanelTabs(left, right) {
  const key = (tab) => `${tab.plugin}\0${tab.id}`
  const a = [...(left?.tabs ?? [])].map(key).sort()
  const b = [...(right?.tabs ?? [])].map(key).sort()
  return a.length === b.length && a.every((each, index) => each === b[index])
}

/**
 * The front panel tab's app document has `selector`. That tab is the only
 * panel frame: the next tab, or another chat, drops it, and a sandbox
 * document still on the wire is reported `net::ERR_ABORTED`.
 */
async function panelAppReady(page, selector) {
  try {
    const { app } = await appFrame(page, "panel", 20_000)
    await app.waitForSelector(selector, { timeout: 20_000 })
    return true
  } catch {
    return false
  }
}

/** The inline app whose document has `selector`, or null once `timeout` passes. */
async function inlineApp(page, selector, timeout = 20_000) {
  const until = Date.now() + timeout
  while (Date.now() < until) {
    const elements = await page.$$(selectorFor.appFrameIn("inline"))
    for (const element of elements) {
      const proxy = await element.contentFrame()
      const app = proxy?.childFrames()[0]
      if (app && !app.isDetached() && (await app.$(selector))) return app
    }
    await sleep(100)
  }
  return null
}

/** C13: a new native context with old SDK history is closed before its first input. */
async function unusedNativeContext(run, step, failures) {
  const native = run.stack.nativeContext
  if (!native) throw new Error("drift requires its focused native context fixture")
  await run.exclusive(async () => {
    let created = false
    if (!native.channel) {
      const model = catalog.models.find((each) => each.modelId === MODELS.claude)
      if (!model) throw new CannotRun("the catalog has no focused Claude model")
      native.channel = await makeChannel(run, failures, {
        name: `native-context-s${run.knobs.seed}`,
        folder: native.workspace,
        model,
      })
      if (!native.channel || failures.length) return
      if (native.channel.folder !== native.workspace)
        throw new Error("focused native channel did not retain its canonical workspace")
      created = true
    } else {
      await run.page
        .locator(`${css.sidebar} nav`)
        .getByText(native.channel.name, { exact: true })
        .first()
        .click({ timeout: 10_000 })
      await openStressConversation(
        run.page,
        native.channel.coordinatorConversationId,
        await titleOf(run, native.channel.coordinatorConversationId),
      )
    }
    const id = native.channel.coordinatorConversationId
    const oldText = prompt(run, STRESS.reply, step, ` native old ${run.engine}`)
    const home = run.page.locator(`${css.focusedPane} ${css.paneHome}`)
    // Creating the channel reserves its coordinator ID. The first UI send
    // materializes that conversation, so there is no snapshot to read yet.
    const previous = created
      ? new Set()
      : new Set(
          (await run.client.conversation.read(id)).messages.map(
            (each) => each.executionId,
          ),
        )
    if (created) {
      await home.locator(css.field).waitFor({ timeout: 15_000 })
      await home.locator(css.field).fill(oldText)
      await home.getByRole(...composerControls.send).click({ timeout: 10_000 })
      const selected = await waitFor(
        () => selectedStressConversation(run.page, oldText),
        30_000,
      )
      if (selected !== id)
        throw new Error("native context step did not send to its channel coordinator")
      run.sent.set(id, (run.sent.get(id) ?? 0) + 1)
    } else await send(run, id, oldText, failures, previous)
    if (!run.checked.includes(id)) run.checked.push(id)
    const { view: before } = await settled(
      run,
      id,
      previous,
      oldText,
      "completed",
      failures,
    )
    if (failures.length || !before) return
    if (before.model !== MODELS.claude)
      throw new Error("native context step did not select Claude")
    const { conversations } = await run.client.conversation.list({})
    if (
      conversations.filter((each) => each.channelId === native.channel.channelId)
        .length !== 1
    )
      throw new Error(
        "focused native workspace has more than its coordinator conversation",
      )
    const title = await titleOf(run, id)
    const reopen = async () => {
      await run.page
        .locator(`${css.sidebar} nav`)
        .getByText(native.channel.name, { exact: true })
        .first()
        .click({ timeout: 10_000 })
      await openStressConversation(run.page, id, title)
    }
    const auditDirectory = join(
      run.stack.gateway.directory,
      "ci",
      "instances",
      INSTANCE,
      "conversations",
      "audit",
    )
    const auditBefore = contextAudit(auditDirectory, id)
    const eventsBefore = nativeContextEvents(native.directory)
    const oldNative = eventsBefore
      .filter((each) => each.method === "session/prompt" && each.outcome === "accepted")
      .at(-1)?.sessionId
    if (!oldNative || !nativeContextExists(native.directory, oldNative))
      throw new Error("focused old input did not establish actual used native history")
    const changed = mkdtempSync(join(run.stack.scratch, "claude-context-changed-"))
    const oldIds = before.messages.map((each) => each.executionId)
    const sameIds = (view) =>
      JSON.stringify(view.messages.map((each) => each.executionId)) ===
      JSON.stringify(oldIds)
    let changedEnvironment = false
    const restart = async (overlay) => {
      await run.restartConsole.during(run.opened, async () => {
        const client = run.client
        run.restarts += 1
        await run.stack.gateway.restart("SIGTERM", overlay ? { overlay } : undefined)
        client.close()
        run.client = await connectClient(run.stack.gateway, INSTANCE, "panel")
        if (!(await run.restartConsole.readable(60_000)))
          throw new Error(
            "native context: browser workspace did not acknowledge restart recovery",
          )
        return true
      })
    }
    const published = async (audit, offset) =>
      waitFor(async () => {
        const events = nativeContextEvents(native.directory).slice(offset)
        const missing = events.find(
          (each) => each.method === "session/resume" && each.outcome === "missing",
        )
        if (missing) return { missing }
        const current = contextAudit(auditDirectory, id)
        if (newContextEvidence(audit.published, current.published).length !== 1)
          return null
        const openings = events.filter(
          (each) => each.method === "session/new" && each.outcome === "opened",
        )
        if (openings.length !== 1) return null
        const view = await run.client.conversation.read(id).catch(() => null)
        return view ? { view, audit: current, sessionId: openings[0].sessionId } : null
      }, 60_000)
    try {
      changedEnvironment = true
      await restart({ CLAUDE_CONFIG_DIR: changed })
      await reopen()
      await run.client.conversation.read(id).catch(() => null)
      const fresh = await published(auditBefore, eventsBefore.length)
      if (!fresh || fresh.missing) {
        failures.push(
          "native context: fingerprint change did not publish one fresh native context",
        )
        return
      }
      if (
        !sameIds(fresh.view) ||
        newContextEvidence(auditBefore.admitted, fresh.audit.admitted).length
      )
        throw new Error("fresh context acquired an input before the cold-reopen boundary")
      if (
        fresh.sessionId === oldNative ||
        nativeContextExists(native.directory, fresh.sessionId)
      )
        throw new Error(
          "fresh unused native context already has history or reused the old identity",
        )
      if (
        nativeContextEvents(native.directory)
          .slice(eventsBefore.length)
          .some((each) => each.method === "session/prompt")
      )
        throw new Error("a prompt reached the fresh context before its cold restart")
      const coldOffset = nativeContextEvents(native.directory).length
      // The same changed directory is retained: this is a cold reopen of the
      // published context, not another fingerprint change.
      await restart()
      const recordColdRefusal = () => {
        if (!run.evidence) return
        writeFileSync(
          join(run.evidence, `native-context-${run.engine}-${step.index}-refusal.json`),
          JSON.stringify(
            {
              conversationId: id,
              oldExecutionIds: oldIds,
              firstUnusedNative: fresh.sessionId,
              firstPhase: nativeContextEvents(native.directory).slice(
                eventsBefore.length,
                coldOffset,
              ),
              coldPhase: nativeContextEvents(native.directory).slice(coldOffset),
              coldAdmissions: newContextEvidence(
                fresh.audit.admitted,
                contextAudit(auditDirectory, id).admitted,
              ),
            },
            null,
            2,
          ),
        )
      }
      // The UI remains the restoration trigger. If native restoration removes
      // its composer, report the exact focused protocol failure instead of
      // attributing the existing opening deadline to an unrelated selector.
      try {
        await reopen()
      } catch (error) {
        const missing = nativeContextEvents(native.directory)
          .slice(coldOffset)
          .some(
            (each) =>
              each.method === "session/resume" &&
              each.outcome === "missing" &&
              each.sessionId === fresh.sessionId,
          )
        if (!missing) throw error
        recordColdRefusal()
        failures.push(
          "native context: cold reopen tried to resume the missing unused native ID",
        )
        return
      }
      await run.client.conversation.read(id).catch(() => null)
      const replacement = await published(fresh.audit, coldOffset)
      if (!replacement || replacement.missing) {
        recordColdRefusal()
        failures.push(
          replacement?.missing
            ? "native context: cold reopen tried to resume the missing unused native ID"
            : "native context: cold reopen did not publish a replacement native context",
        )
        return
      }
      if (
        replacement.sessionId === fresh.sessionId ||
        !sameIds(replacement.view) ||
        newContextEvidence(fresh.audit.admitted, replacement.audit.admitted).length
      )
        throw new Error(
          "cold replacement changed retained executions or admitted work before Send",
        )
      const coldEvents = nativeContextEvents(native.directory).slice(coldOffset)
      if (
        coldEvents.some(
          (each) => each.method === "session/resume" || each.method === "session/prompt",
        )
      )
        throw new Error("cold unused context was resumed or prompted before Send")
      const newText = prompt(run, STRESS.reply, step, ` native new ${run.engine}`)
      const known = await send(run, id, newText, failures, new Set(oldIds))
      const { view: after } = await settled(
        run,
        id,
        known,
        newText,
        "completed",
        failures,
      )
      if (failures.length || !after) return
      const added = after.messages.filter((each) => !oldIds.includes(each.executionId))
      if (
        after.messages.length !== before.messages.length + 1 ||
        added.length !== 1 ||
        added[0].userText !== newText ||
        added[0].status !== "completed" ||
        JSON.stringify(
          after.messages.slice(0, oldIds.length).map((each) => each.executionId),
        ) !== JSON.stringify(oldIds)
      )
        failures.push(
          "native context: cold reopen did not retain old executions and complete exactly one new input",
        )
      const pane = run.page.locator(css.focusedPane)
      const count = async (role, text) =>
        pane
          .locator(css.message)
          .filter({ hasText: text })
          .evaluateAll(
            (rows, role) =>
              rows.filter((row) => row.getAttribute("data-role") === role).length,
            role,
          )
      const shown = await waitFor(
        async () =>
          (await count("user", oldText)) === 1 &&
          (await count("agent", `${SAYS.echo}${oldText}`)) === 1 &&
          (await count("user", newText)) === 1 &&
          (await count("agent", `${SAYS.echo}${newText}`)) === 1,
        15_000,
      )
      if (!shown)
        failures.push(
          "native context: focused pane does not show the retained old exchange and exactly one new reply",
        )
      const finalEvents = nativeContextEvents(native.directory).slice(coldOffset)
      const prompts = finalEvents.filter((each) => each.method === "session/prompt")
      if (
        prompts.length !== 1 ||
        prompts[0].sessionId !== replacement.sessionId ||
        !nativeContextExists(native.directory, replacement.sessionId)
      )
        failures.push(
          "native context: replacement did not receive exactly the new prompt with persisted native history",
        )
      if (run.evidence) {
        writeFileSync(
          join(run.evidence, `native-context-${run.engine}-${step.index}.json`),
          JSON.stringify(
            {
              conversationId: id,
              oldExecutionIds: oldIds,
              newExecutionId: added[0]?.executionId,
              oldNative,
              firstUnusedNative: fresh.sessionId,
              replacementNative: replacement.sessionId,
              firstPhase: nativeContextEvents(native.directory).slice(
                eventsBefore.length,
                coldOffset,
              ),
              coldPhase: finalEvents,
              focusedPane: shown,
            },
            null,
            2,
          ),
        )
        await pane.screenshot({
          path: join(run.evidence, `native-context-${run.engine}-${step.index}.png`),
          scale: "css",
        })
      }
    } finally {
      if (changedEnvironment) await restart({ CLAUDE_CONFIG_DIR: native.baseline })
      await run.page
        .locator(`${css.sidebar} nav`)
        .getByText("Conversations", { exact: true })
        .first()
        .click({ timeout: 10_000 })
      await openStressConversation(run.page, run.main.id, run.main.title)
    }
  })
}

/** The enrolled-peer example (`read_only_sync`), built beside the gateway. */
function peerExampleBin() {
  const bin = join(repoRoot, "target/debug/examples/read_only_sync")
  if (!existsSync(bin))
    throw new Error(
      "the peer example is not built (cargo build -p nessa-client-core --example read_only_sync)",
    )
  return bin
}

/** The last JSON object `read_only_sync` wrote. Diagnostics stay on stderr. */
function peerOutcome(text) {
  const lines = text.trim().split("\n")
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    if (!lines[index].startsWith("{")) continue
    try {
      return JSON.parse(lines[index])
    } catch {
      return null
    }
  }
  return null
}

/** Run one peer-example command. `input` is the enrollment code for `pair`. */
function peerExample(args, input) {
  const result = spawnSync(peerExampleBin(), args, {
    input,
    encoding: "utf8",
    timeout: 60_000,
  })
  return {
    status: result.status,
    outcome: peerOutcome(result.stdout ?? ""),
    stderr: (result.stderr ?? "").trim(),
    error: result.error,
  }
}

/**
 * Enroll a peer gateway against `invitationId` with `code`, approving the
 * claimed key while `pair` holds the invitation. The credential id is the
 * one `status` names once the enrollment is active.
 */
async function enrollPeer(run, profile, code, invitationId) {
  const child = spawn(peerExampleBin(), ["pair", profile, "peer"], {
    stdio: ["pipe", "pipe", "pipe"],
  })
  let stdout = ""
  let stderr = ""
  child.stdout.setEncoding("utf8")
  child.stderr.setEncoding("utf8")
  child.stdout.on("data", (chunk) => {
    stdout += chunk
  })
  child.stderr.on("data", (chunk) => {
    stderr += chunk
  })
  child.stdin.end(`${code}\n`)
  const exited = new Promise((resolve) => {
    child.once("exit", (status) => resolve(status))
  })
  const streamsClosed = Promise.all([
    new Promise((resolve) => child.stdout.on("close", resolve)),
    new Promise((resolve) => child.stderr.on("close", resolve)),
  ])
  try {
    const deviceKey = await waitFor(async () => {
      try {
        return (await run.client.pairing.status(invitationId)).claimedDeviceKey ?? false
      } catch {
        return false
      }
    }, 30_000)
    if (!deviceKey)
      throw new Error(
        `the peer never claimed its invitation: ${stderr.trim() || stdout.trim()}`,
      )
    const approved = await run.client.pairing.approve(invitationId, deviceKey)
    if (approved.activationStopped)
      throw new Error(
        `the peer's approval stopped ${approved.activationStopped}: ${approved.status.phase}`,
      )
    const status = await Promise.race([exited, sleep(30_000).then(() => "timeout")])
    if (status === "timeout")
      throw new Error(`the peer's enrollment did not finish: ${stderr.trim()}`)
    await streamsClosed
    let enrollment = peerOutcome(stdout)?.enrollment
    for (let tries = 0; enrollment?.phase !== "active" && tries < 40; tries += 1) {
      await sleep(250)
      const read = peerExample(["status", profile])
      enrollment = read.outcome?.enrollment ?? enrollment
      if (tries === 39 && enrollment?.phase !== "active")
        throw new Error(
          `the peer did not become active (${enrollment?.phase ?? "no status"}): ${read.stderr || stderr.trim()}`,
        )
    }
    if (!enrollment?.credentialId)
      throw new Error(
        `the peer enrollment named no credential: ${JSON.stringify(enrollment)}`,
      )
    return enrollment
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL")
    await exited
  }
}

/**
 * The `launch` step's slow launch: the host's startup outlasts its hold of
 * the window's read, so the window is refused as still starting. True when
 * the window stayed uncovered and listed the main conversation once up.
 */
async function slowLaunch(run, failures) {
  const page = run.page
  const removeKey = () =>
    page.evaluate((key) => sessionStorage.removeItem(key), STARTING.key)
  await page.evaluate(
    ([key, hold]) => sessionStorage.setItem(key, String(hold)),
    [STARTING.key, SLOW_LAUNCH_HOLD_MS],
  )
  let covered
  let refused
  try {
    await page.reload()
    // Wait until the host has refused at least once as still starting.
    const passed = await page
      .waitForFunction(() => window.__fakeHostStillStarting > 0, null, {
        timeout: SLOW_LAUNCH_HOLD_MS + LAUNCH_STARTING_MS,
      })
      .then(() => true)
      .catch(() => false)
    refused = passed
    // And a little after: the refusal has reached the window by now.
    covered = await page
      .locator(css.startupScreen)
      .waitFor({ timeout: LAUNCH_STARTING_MS })
      .then(() => true)
      .catch(() => false)
  } finally {
    await removeKey().catch(() => removeKey().catch(() => {}))
  }
  if (!refused) {
    failures.push("the slow launch's startup never outlasted the host's hold")
    return false
  }
  if (covered) {
    failures.push(
      "the window showed the startup screen for a startup still running past the host's hold",
    )
    return false
  }
  const listed = await page
    .locator(selectorFor.sessionListRowFor(run.main.id))
    .waitFor({ timeout: LAUNCH_LISTED_MS })
    .then(() => true)
    .catch(() => false)
  if (!listed) {
    failures.push(
      `after a slow launch the window did not list the main conversation within ${LAUNCH_LISTED_MS}ms of the gateway being up`,
    )
    return false
  }
  if ((await page.locator(css.startupScreen).count()) > 0) {
    failures.push("the startup screen covers the window after a slow launch was up")
    return false
  }
  return true
}

const unsignedSwitch = (...args) => steps.unsignedSwitchBody(...args)
const failedModelControl = (...args) => steps.failedControlBody(...args)
const stuckModelControl = (...args) => steps.stuckControlBody(...args)

const steps = {
  /** The cloud for new thread VMs is picked in Settings › Connections ›
   * Accounts: a VM made while Agent37 is chosen is made on Agent37 (the
   * fixture's fake one), and its thread stays there after Boat is chosen
   * again, while a new thread goes to Boat. */
  async cloud(run, step, failures) {
    const original = run.main
    const fixture = run.stack.vmFixture
    const choose = async (cloud) => {
      await openAccounts(run.page)
      await unlockRequestedVault(run.client, run.page)
      const connected = await run.page
        .locator('[data-agent37-state="connected"]')
        .waitFor({ timeout: 10_000 })
        .then(
          () => true,
          () => false,
        )
      if (!connected) failures.push("Settings does not show Agent37 connected")
      const row = run.page.locator("[data-cloud-choice]")
      await pickChoice(run.page, row, cloud)
      const shown = await waitFor(
        async () => (await row.getAttribute("data-cloud-choice")) === cloud,
        5_000,
      )
      if (!shown) failures.push(`the cloud choice does not show ${cloud} once picked`)
      const saved = (await run.client.cloud.read()).cloud
      if (saved !== cloud)
        failures.push(`the gateway kept ${saved} after ${cloud} was picked`)
      await closeSettings(run.page)
    }
    const made = (id) =>
      fixture
        .events()
        .filter((event) => event.thread === id && event.kind === "created")
        .map((event) => event.cloud)
    const talk = async (id, said) => {
      await run.client.conversation.submit(run.client.conversation.prepare(id, said))
      await settled(run, id, new Set(), said, "completed", failures)
    }
    const open = async (id) => {
      await run.client.conversation.create({
        conversationId: id,
        agent: run.stack.agent,
        environment: "boat:thread",
      })
    }
    await choose("agent37")
    const onAgent37 = randomUUID()
    await open(onAgent37)
    await talk(onAgent37, `${STRESS.reply} cloud agent37 ${step.index}`)
    if (made(onAgent37).join() !== "agent37")
      failures.push(
        `a VM made with Agent37 chosen was made on: ${made(onAgent37).join()}`,
      )
    await choose("boat")
    // Rested once idle, the thread's next turn opens its VM again: on the
    // cloud it was made on, whatever is chosen now.
    const events = () => fixture.events().filter((event) => event.thread === onAgent37)
    let rested = -1
    if (
      !(await waitFor(
        () =>
          (rested = events().findIndex(
            (event) => event.kind === "rested" && event.cloud === "agent37",
          )) >= 0,
        (PARK_IDLE_SECONDS + 30) * 1000,
      ))
    )
      failures.push("the thread on Agent37 was not rested on Agent37 once idle")
    await talk(onAgent37, `${STRESS.reply} cloud stays ${step.index}`)
    const after = events().slice(rested + 1)
    if (!after.some((event) => event.kind === "connected" && event.cloud === "agent37"))
      failures.push(
        "the thread on Agent37 was not opened again on Agent37 after its rest",
      )
    if (made(onAgent37).join() !== "agent37" || events().some((e) => e.cloud === "boat"))
      failures.push(
        `a thread on Agent37 reached Boat after Boat was chosen: ${JSON.stringify(events())}`,
      )
    const onBoat = randomUUID()
    await open(onBoat)
    await talk(onBoat, `${STRESS.reply} cloud boat ${step.index}`)
    if (made(onBoat).join() !== "boat")
      failures.push(`a VM made with Boat chosen was made on: ${made(onBoat).join()}`)
    // A channel names its own cloud: its threads go to Agent37 while Boat is
    // the cloud chosen for new thread VMs, and the channel says so back.
    const name = `stress-cloud-r${step.index}-s${run.knobs.seed}`
    // Made here as setup; the person moves its threads to Agent37 in the
    // channel's settings, from the menu that lists every cloud.
    const channel = await run.client.channels.create({
      requestId: name,
      name,
      agent: run.stack.agent,
      model: run.stack.profile.model,
    })
    const inChannel = randomUUID()
    try {
      await run.page.reload()
      await unlockRequestedVault(run.client, run.page)
      const named = run.page
        .locator(`${css.sidebar} nav`)
        .getByText(name, { exact: true })
        .first()
      await named.hover({ timeout: 15_000 })
      await run.page
        .locator(
          `${css.channelSettingsOpen}[data-channel-settings-open="${channel.channelId}"]`,
        )
        .click()
      const control = run.page
        .locator(css.channelSettingsDialog)
        .locator(`[data-channel-placement="${channel.channelId}"]`)
      try {
        await pickChoice(
          run.page,
          control.locator('[data-placement-row="threads"]'),
          "agent37",
        )
        if (
          !(await waitFor(
            async () =>
              (await control.getAttribute("data-thread-placement")) === "agent37",
            10_000,
          ))
        )
          failures.push("the channel settings do not show new threads on Agent37")
        if (run.evidence)
          await run.page.screenshot({
            path: join(run.evidence, `cloud-${run.engine}-channel-agent37.png`),
          })
      } finally {
        const dialog = run.page.locator(css.channelSettingsDialog)
        for (let tries = 0; tries < 3 && (await dialog.count()) > 0; tries += 1) {
          await run.page.keyboard.press(keys.escape)
          await dialog.waitFor({ state: "detached", timeout: 2_000 }).catch(() => {})
        }
      }
      const { channels } = await run.client.channels.list()
      const listed = channels.find((each) => each.channelId === channel.channelId)
      if (listed?.threadPlacement !== "agent37")
        failures.push(`a channel moved to Agent37 reads back ${listed?.threadPlacement}`)
      await run.client.conversation.create({
        conversationId: inChannel,
        agent: run.stack.agent,
        channelId: channel.channelId,
      })
      await talk(inChannel, `${STRESS.reply} cloud channel ${step.index}`)
      if (made(inChannel).join() !== "agent37")
        failures.push(
          `a thread in a channel on Agent37 was made on: ${made(inChannel).join()}`,
        )
      run.checked.push(inChannel)
    } finally {
      await run.client.channels
        .setPlacement({
          requestId: `${name}-back`,
          channelId: channel.channelId,
          coordinatorPlacement: "local",
          threadPlacement: "local",
        })
        .catch((error) =>
          failures.push(`${name} did not go back here: ${explain(error)}`),
        )
    }
    run.checked.push(onAgent37, onBoat)
    run.main = original
    await openStressConversation(run.page, original.id, original.title)
  },
  async vmBackground(run, step, failures) {
    const original = run.main
    const id = randomUUID()
    const setup = `${STRESS.reply} VM background ${step.index}`
    // Boat makes the VM but its answer is lost: the opening is refused, and
    // the next one asks Boat again with the same key and gets that same VM
    // back instead of leaving the thread with none or making a second one.
    writeFileSync(join(run.stack.vmFixture.root, `lose-next-create-${id}`), "lose")
    const create = () =>
      run.client.conversation
        .create({
          conversationId: id,
          agent: run.stack.agent,
          environment: "boat:thread",
        })
        .then(
          () => null,
          (error) => error,
        )
    if (!(await create()))
      failures.push("a conversation whose VM creation was lost was not refused")
    const retried = await create()
    if (retried)
      failures.push(`the opening after a lost VM creation failed: ${retried.message}`)
    await run.client.conversation.submit(run.client.conversation.prepare(id, setup))
    await settled(run, id, new Set(), setup, "completed", failures)
    const made = run.stack.vmFixture
      .events()
      .filter((event) => event.thread === id)
      .map((event) => event.kind)
    if (
      made
        .filter((kind) => kind.startsWith("create"))
        .slice(0, 3)
        .join(",") !== "created,create_lost,create_replayed" ||
      made.filter((kind) => kind === "created").length !== 1
    )
      failures.push(
        `a lost VM creation was not finished by the next opening: ${made.join(", ")}`,
      )
    run.checked.push(id)
    run.main = { id, title: await titleOf(run, id) }
    await openStressConversation(run.page, id, run.main.title)
    run.backgroundPrefix = STRESS.vmBackground
    let launchIndex = -1
    let leaseEvidence = null
    run.backgroundRunning = async (task, initialLease) => {
      if (
        !(await waitFor(
          async () =>
            (launchIndex = run.stack.vmFixture
              .events()
              .findIndex(
                (event) => event.thread === id && event.kind === "background",
              )) >= 0,
          20000,
        ))
      ) {
        failures.push("the admitted VM command never reached its workspace runtime")
        return
      }
      await sleep((PARK_IDLE_SECONDS + 1) * 1000)
      const all = run.stack.vmFixture.events()
      const events = all.filter((event) => event.thread === id)
      if (events.filter((event) => event.kind === "background").length !== 1)
        failures.push(
          "the VM task did not dispatch exactly once through its workspace runtime",
        )
      if (
        all
          .slice(launchIndex + 1)
          .some((event) => event.thread === id && event.kind === "rested")
      )
        failures.push(
          "the VM rested while its background command was still running past idle",
        )
      const held = await run.client.conversation.read(id)
      leaseEvidence = { initial: initialLease, afterIdle: held.lease }
      if (
        held.lease?.state !== "live" ||
        held.lease.environment !== "boat" ||
        held.lease.revision !== initialLease?.revision
      )
        failures.push(
          "the cloud agent parked or changed its lease while its background task was running",
        )
      if (
        !held.tools.some(
          (tool) =>
            tool.toolId === task.toolId &&
            tool.executionId === task.executionId &&
            tool.status === "running",
        )
      )
        failures.push("the VM background row did not stay running past idle")
      writeFileSync(join(run.stack.vmFixture.root, id, "finish-background"), "release")
    }
    try {
      await steps.background(run, step, failures)
      const view = await run.client.conversation.read(id)
      if (
        !view.tools.some((tool) =>
          tool.details?.includes(join(run.stack.vmFixture.root, id)),
        )
      )
        failures.push(
          "the VM background command did not run in its admitted VM working directory",
        )
      if (
        !(await waitFor(
          async () =>
            launchIndex >= 0 &&
            run.stack.vmFixture
              .events()
              .slice(launchIndex + 1)
              .some((event) => event.thread === id && event.kind === "rested"),
          30000,
        ))
      )
        failures.push("the completed VM background task did not permit idle rest")
      // Boat fails the rested VM's next opening. Sent from the composer, the
      // message is shown not sent, and the workspace records why, as one of
      // the causes a person acts on. Send Again then opens the VM and the
      // message goes through. A read of the rested conversation would open
      // its VM itself, so its turns are the ones read before it rested.
      const after = `${STRESS.reply} VM open fails ${step.index}`
      const seen = new Set(view.messages.map((each) => each.executionId))
      writeFileSync(join(run.stack.vmFixture.root, id, "fail-next-open"), "fail")
      await send(run, id, after, failures, seen)
      const unsent = run.page.locator("[data-send-failure]")
      if (!(await waitFor(async () => (await unsent.count()) === 1, 60_000)))
        failures.push("a send whose VM could not be opened was not shown as not sent")
      else if (!(await unsent.textContent())?.includes("Error sending"))
        failures.push(
          `a send whose VM could not be opened says ${JSON.stringify(await unsent.textContent())}`,
        )
      if (
        !run.stack.vmFixture
          .events()
          .some(
            (event) =>
              event.thread === id &&
              event.kind === "open_refused" &&
              event.cause === "provider_failed" &&
              event.failure === "provider",
          )
      )
        failures.push("a failed VM opening was not recorded with its cause")
      if ((await unsent.count()) === 1) {
        await unsent.getByRole("button", { name: "Send Again" }).click()
        await settled(run, id, seen, after, "completed", failures)
        if (!(await waitFor(async () => (await unsent.count()) === 0, 10_000)))
          failures.push("the message sent again still shows as not sent")
      }
      if (run.evidence)
        writeView(run.evidence, `views/vm-background-${run.engine}-${step.index}.json`, {
          lease: leaseEvidence,
          events: run.stack.vmFixture.events().filter((event) => event.thread === id),
        })
    } finally {
      run.backgroundPrefix = null
      run.backgroundRunning = null
      run.main = original
      await openStressConversation(run.page, original.id, original.title)
    }
  },
  async background(run, step, failures) {
    const text = prompt(run, run.backgroundPrefix ?? STRESS.background, step)
    const seen = await send(run, run.main.id, text, failures)
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    if (!turn) return
    await expectShown(run.page, SAYS.backgroundStarted, failures)
    const running = await run.client.conversation.read(run.main.id)
    const task = running.tools.find(
      (tool) => tool.executionId === turn.executionId && tool.backgroundTaskId,
    )
    if (!task || task.status !== "running") {
      failures.push(
        "the background tool did not remain running after its calling turn ended",
      )
      return
    }
    const row = run.page
      .locator('[data-slot="tool-trace-step"]')
      .filter({ hasText: "Background task" })
      .last()
    if (
      !(await waitFor(
        async () =>
          (await row.getAttribute("data-status").catch(() => null)) === "running",
        5000,
      ))
    )
      failures.push("the background task is not running in the existing waterfall")
    const trace = run.page
      .locator('[data-slot="tool-trace"]')
      .filter({ hasText: "Background task" })
      .last()
    if (run.evidence)
      await trace.screenshot({
        path: join(run.evidence, `background-${run.engine}-running.png`),
      })
    await run.backgroundRunning?.(task, running.lease)
    const completed = await waitFor(async () => {
      const view = await run.client.conversation.read(run.main.id)
      const original = view.tools.find(
        (tool) => tool.toolId === task.toolId && tool.executionId === task.executionId,
      )
      const continuation = view.messages.find(
        (message) =>
          message.executionId === `background-${task.backgroundTaskId}` && ended(message),
      )
      return original?.status === "completed" && continuation
        ? { view, original, continuation }
        : null
    }, 30000)
    if (!completed) {
      failures.push(
        "background completion did not settle the original tool and resume the conversation",
      )
      return
    }
    if (!completed.original.details.includes("background-stress-output"))
      failures.push("background task lost its actual output")
    if (completed.continuation.userText !== "")
      failures.push("background continuation leaked into a user message")
    if (
      completed.view.messages.filter(
        (message) => message.executionId === completed.continuation.executionId,
      ).length !== 1
    )
      failures.push("background completion created duplicate continuation turns")
    await expectShown(run.page, SAYS.backgroundFinished, failures)
    if (
      !(await waitFor(
        async () =>
          (await trace.getAttribute("data-status").catch(() => null)) === "complete",
        5000,
      ))
    )
      failures.push("the background waterfall did not settle on completion")
    if (run.evidence)
      await trace.screenshot({
        path: join(run.evidence, `background-${run.engine}-finished.png`),
      })
  },
  async inputFiles(run, step, failures) {
    await uploadedInputFiles(run, step, {
      prefix: STRESS.reply,
      settled: (text) =>
        settled(run, run.main.id, new Set(), text, "completed", failures),
      shown: (name) => expectShown(run.page, name, failures, 10000),
    })
  },
  async pairedCatalogue(run, step) {
    const result = await pairedCatalogue(step.cycles)
    if (run.evidence)
      writeFileSync(
        join(run.evidence, `paired-catalogue-${run.engine}-${step.index}.json`),
        JSON.stringify(result, null, 2),
      )
  },
  async model(run, step, failures) {
    const picker = dock(run.page).locator(css.modelChip)
    // A mode switch just before this step can still be settling. The picker
    // is disabled only while that change is in flight.
    const enabled = await waitFor(async () => !(await picker.isDisabled()), 10_000)
    if (!enabled) {
      failures.push("the established conversation's model picker is disabled")
      return
    }
    const before = await readWithReviews(run.client, run.main.id)
    const current = catalog.models.find(
      (model) => model.modelId === before.runtime?.model,
    )
    const target =
      current &&
      catalog.models.find(
        (model) =>
          model.provider === current.provider && model.modelId !== current.modelId,
      )
    if (!target)
      throw new CannotRun("no second catalog model in this conversation's provider")
    const text = prompt(run, STRESS.permission, step)
    const seen = await send(run, run.main.id, text, failures)
    if (!(await card(run.page))) {
      failures.push("no original review before model change")
      return
    }
    const waiting = await readWithReviews(run.client, run.main.id)
    const original = waiting.permissions.map((review) => ({
      review: review.review,
      executionId: review.executionId,
      options: review.options,
    }))
    const draft = prompt(run, STRESS.model, step)
    const box = dock(run.page).locator("textarea")
    await box.fill(draft)
    await picker.click()
    const photograph = run.evidence && !run.modelPhotographed
    if (photograph)
      await run.page.screenshot({
        path: join(run.evidence, `model-${run.engine}-picker.png`),
        scale: "css",
      })
    await run.page.locator(css.modelSearch).fill(target.displayName)
    await run.page
      .locator(css.modelOption)
      .filter({
        has: run.page
          .locator(css.modelOptionLabel)
          .getByText(target.displayName, { exact: true }),
      })
      .locator(css.modelOptionLabel)
      .click()
    // Picking only stages the model: the picker shows it, and the gateway is
    // asked for nothing until the next message is sent.
    if (
      !(await waitFor(
        async () =>
          (await picker.getAttribute("aria-label"))?.includes(target.displayName),
        5_000,
      ))
    )
      failures.push("the picker did not show the staged model")
    const asked = await waitFor(async () => {
      const view = await readWithReviews(run.client, run.main.id)
      return view.runtime?.model !== current.modelId || view.modelChange ? view : null
    }, 2_000)
    if (asked) {
      failures.push("picking a model changed the conversation before a message was sent")
      return
    }
    const staged = await readWithReviews(run.client, run.main.id)
    if (photograph) {
      await run.page.screenshot({
        path: join(run.evidence, `model-${run.engine}-staged-review-draft.png`),
        scale: "css",
      })
      run.modelPhotographed = true
    }
    const unchanged = staged.permissions.map((review) => ({
      review: review.review,
      executionId: review.executionId,
      options: review.options,
    }))
    if (JSON.stringify(original) !== JSON.stringify(unchanged))
      failures.push("model change replaced the original review or its choices")
    if ((await box.inputValue()) !== draft)
      failures.push("model change lost the unsent draft")
    if (
      !(await dock(run.page)
        .getByRole(...composerControls.stop)
        .count())
    )
      failures.push("model change removed Stop while its original turn waits")
    await answerCard(run, run.main.id, "allow", failures, "Report the rows")
    await settled(run, run.main.id, seen, text, "completed", failures)
    if ((await box.inputValue()) !== draft)
      failures.push("answering the original review lost the model-change draft")
    // Sending applies the staged model first, and the message runs on it.
    const next = await send(run, run.main.id, draft, failures)
    const confirmed = await waitFor(async () => {
      const view = await readWithReviews(run.client, run.main.id)
      return view.runtime?.model === target.modelId && !view.modelChange ? view : null
    }, 15_000)
    if (!confirmed) {
      failures.push("sending did not apply the staged model")
      return
    }
    await settled(run, run.main.id, next, draft, "completed", failures)
    await expectShown(run.page, `${SAYS.model}${target.modelId}`, failures)
    if (run.evidence)
      writeFileSync(
        join(run.evidence, `model-${run.engine ?? "window"}-${step.index}.json`),
        JSON.stringify(
          {
            before: current.modelId,
            after: target.modelId,
            original,
            confirmed: confirmed.runtime,
            effect: confirmed.modelChangeEffect,
          },
          null,
          2,
        ),
      )
  },
  /**
   * The model and thinking picker: a star lists a model under Starred, where
   * the picker then opens; one press of a bar stages that model at that
   * level, which the chip names, and closes the picker; Fast beside the chip
   * is offered only for a model that has it; and the next message runs on
   * the model chosen. The star is taken off again.
   */
  async picker(run, step, failures) {
    await clearStarredModels(run.page)
    try {
      await pickerStep(run, step, failures)
    } finally {
      await clearStarredModels(run.page).catch(() => {})
    }
  },

  async reply(run, step, failures, sendBy) {
    const text = prompt(run, STRESS.reply, step, step.extra ?? "")
    const seen = await send(
      run,
      run.main.id,
      text,
      failures,
      undefined,
      dock(run.page),
      sendBy,
    )
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    // A reply carried on behind a failure echoes the notice the agent was
    // given first; the person's own message is the text they sent.
    const echoed = turn?.retried ? "[" : text
    await expectShown(run.page, `${SAYS.echo}${echoed}`, failures)
    if ((await run.page.locator(css.sendFailure).count()) > 0)
      failures.push("the composer shows a send failure")
    // The message after a sign-in refusal supersedes it (ADR 501).
    if (
      step.after === "signIn" &&
      (await run.page.locator(".provider-sign-in").count()) > 0
    )
      failures.push("the sign-in card stayed after the next message")
  },

  async permission(run, step, failures) {
    const text = prompt(run, STRESS.permission, step)
    const seen = await send(run, run.main.id, text, failures)
    if (step.mode === "full") {
      // Full access reaches the agent as its own mode: it asks for nothing.
      const { view } = await settled(run, run.main.id, seen, text, "completed", failures)
      if (view?.permissions?.length) failures.push("a review is open under full access")
      await expectShown(run.page, SAYS.allowed, failures)
      return
    }
    await answerCard(run, run.main.id, step.answer, failures, "Report the rows")
    await settled(run, run.main.id, seen, text, "completed", failures)
    await expectShown(
      run.page,
      step.answer === "allow" ? SAYS.allowed : SAYS.denied,
      failures,
    )
  },

  async waitingRow(run, step, failures) {
    const text = prompt(run, STRESS.permission, step)
    const seen = await send(run, run.main.id, text, failures)
    // The conversation asks; the session list only lists it under Needs you.
    const group = run.page.locator('[data-list-group="needs-you"]')
    const row = group.locator(`[data-session-row="${run.main.id}"]`)
    const listed = await row
      .waitFor({ timeout: 30_000 })
      .then(() => true)
      .catch(() => false)
    if (!listed) {
      failures.push("the waiting conversation is not listed under Needs you")
      return
    }
    // A plain row: nothing of the request, no answers, no form beside it.
    const drawn = await group.evaluate((element) => ({
      role: element.getAttribute("role"),
      controls: [...element.querySelectorAll("button, input, textarea")]
        .filter((control) => !control.closest("[data-session-row]"))
        .map((control) => control.getAttribute("aria-label") ?? control.textContent),
      asks: element.querySelectorAll(".workspace-approval-command, .workspace-question")
        .length,
    }))
    if (drawn.role !== "listbox") failures.push(`Needs you is a ${drawn.role}`)
    if (drawn.controls.length || drawn.asks)
      failures.push(
        `Needs you draws the request beside its row: ${JSON.stringify(drawn.controls)}`,
      )
    // Opened from its row, the chat asks, and is answered there.
    await row.click()
    await answerCard(run, run.main.id, step.answer, failures, "Report the rows")
    await settled(run, run.main.id, seen, text, "completed", failures)
    await expectShown(
      run.page,
      step.answer === "allow" ? SAYS.allowed : SAYS.denied,
      failures,
    )
    if (!(await waitFor(async () => (await row.count()) === 0, 10_000)))
      failures.push("the answered row is still in Needs you")
  },

  async stream(run, step, failures) {
    const text = prompt(run, STRESS.stream, step)
    // Every task the window runs for 50 ms or more while the reply streams.
    await run.page.evaluate(() => {
      window.__stressLongTasks = []
      window.__stressLongObserver?.disconnect()
      window.__stressLongObserver = new PerformanceObserver((list) => {
        for (const entry of list.getEntries())
          window.__stressLongTasks.push(entry.duration)
      })
      if (PerformanceObserver.supportedEntryTypes?.includes("longtask"))
        window.__stressLongObserver.observe({ type: "longtask" })
      else window.__stressLongTasks = null
    })
    const seen = await send(run, run.main.id, text, failures)
    const last = streamWord(STREAM_WORDS - 1).trim()
    // Live: the reply's newest words are drawn while the turn still runs,
    // though it streamed more chunks and text than a message keeps.
    if (!(await shows(run.page, last, STREAM_HOLD_MS + 20_000)))
      failures.push(`while streaming, the window never showed the newest words (${last})`)
    else if ((await pageText(run.page)).includes(fold(SAYS.streamEnd)))
      failures.push("the newest words were drawn only after the turn ended")
    const long = await run.page.evaluate(() => {
      window.__stressLongObserver?.disconnect()
      return window.__stressLongTasks
    })
    if (!long) console.log(`stream: long tasks are not measured in ${run.engine}`)
    else
      console.log(
        `stream: ${long.length} tasks of 50 ms or more while ${STREAM_WORDS} words streamed, longest ${Math.round(Math.max(0, ...long))} ms, ${Math.round(long.reduce((sum, each) => sum + each, 0))} ms in all`,
      )
    // The sidebar closed and opened again draws the same reply, not an
    // emptied one.
    await run.page.keyboard.press(keys.toggleSidebar)
    await sleep(400)
    await run.page.keyboard.press(keys.toggleSidebar)
    if (!(await shows(run.page, last, 10_000)))
      failures.push("after the sidebar closed and opened, the newest words are gone")
    let { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    await expectShown(run.page, SAYS.streamEnd, failures)
    const whole = `${SAYS.stream}${text}${Array.from(
      { length: STREAM_WORDS },
      (_, index) => streamWord(index),
    ).join("")} ${SAYS.streamEnd}`
    const reply = (view) => {
      const answer = view.messages.find((each) => each.executionId === turn?.executionId)
      return {
        answer,
        said: (answer?.parts ?? []).map((part) => part.text ?? "").join(""),
      }
    }
    // The main chat holds more than this reply, so its view may begin partway
    // through it: what it must hold is the reply's end, the newest words.
    const { said: newest } = reply(await run.client.conversation.read(run.main.id))
    if (!newest || !whole.endsWith(newest))
      failures.push(
        `the main chat's read reply does not end with its newest words: ${JSON.stringify(newest.slice(-40))}`,
      )
    // A chat of this reply alone holds all of it: a fresh read, checked as the
    // window checks a view, holds every word in order, joined into a few parts.
    const conversationId = randomUUID()
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    run.checked.push(conversationId)
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, text),
    )
    const alone = await settled(
      run,
      conversationId,
      new Set(),
      text,
      "completed",
      failures,
    )
    turn = alone.turn
    const { answer, said } = reply(await run.client.conversation.read(conversationId))
    if (said !== whole)
      failures.push(
        `the read reply is ${said.length} of ${whole.length} bytes, ending ${JSON.stringify(said.slice(-40))}`,
      )
    if ((answer?.parts.length ?? 0) > 4)
      failures.push(`the reply is ${answer?.parts.length} parts, not joined`)
  },

  async reviews(run, step, failures) {
    const text = prompt(run, STRESS.reviews, step)
    const seen = await send(run, run.main.id, text, failures)
    const count = run.knobs.reviews
    for (let n = 1; n <= count && failures.length === 0; n += 1)
      await answerCard(run, run.main.id, "allow", failures, `Review ${n} of ${count}`)
    await settled(run, run.main.id, seen, text, "completed", failures, 120)
    await expectShown(run.page, SAYS.reviewed, failures)
  },

  async large(run, step, failures) {
    const text = prompt(run, STRESS.large, step)
    const seen = await send(run, run.main.id, text, failures)
    await answerCard(run, run.main.id, step.answer, failures, "A large review")
    await settled(run, run.main.id, seen, text, "completed", failures)
    await expectShown(
      run.page,
      step.answer === "allow" ? SAYS.allowed : SAYS.denied,
      failures,
    )
  },

  async stop(run, step, failures) {
    const text = prompt(run, STRESS.wait, step)
    const seen = await send(run, run.main.id, text, failures)
    await expectShown(run.page, SAYS.waiting, failures)
    if (!(await stopShown(run.page))) {
      failures.push("Send did not turn into Stop while the turn ran")
      return
    }
    await oneWorkingSign(run, run.main.id, failures)
    await channelRunning(run.page, MAIN_CHANNEL, true, failures)
    await dock(run.page)
      .getByRole(...composerControls.stop)
      .click({ timeout: 10_000 })
    await settled(run, run.main.id, seen, text, "cancelled", failures)
    if (!(await sendShown(run.page))) failures.push("Stop did not turn back into Send")
    // Background turns run in the same channel and keep it running: the
    // channel is idle only once they are drained and held.
    await run.exclusive(() => channelRunning(run.page, MAIN_CHANNEL, false, failures))
  },

  async "parent-stop"(run, step, failures) {
    const text = prompt(run, STRESS.parentStop, step)
    const seen = await send(run, run.main.id, text, failures)
    const owned = (turn) => {
      const said = (turn?.parts ?? []).map((part) => part.text ?? "").join("")
      const line = said.slice(said.indexOf(SAYS.owned) + SAYS.owned.length)
      return said.includes(SAYS.owned)
        ? line.match(/^([0-9a-f-]{36}) ([0-9a-f-]{36})/i)
        : null
    }
    const parent = await untilTurn(
      run,
      run.main.id,
      { text },
      (turn) => turn.status === "running" && owned(turn),
    )
    const childIds = owned(parent.turn)
    if (!childIds || parent.turn?.status !== "running") {
      failures.push("the parent did not identify its owned child seed")
      return
    }
    run.owned.add(childIds[1])
    const seedExecution = childIds[2]
    const seed = await untilTurn(
      run,
      childIds[1],
      0,
      (turn) => turn.executionId === seedExecution && turn.status === "completed",
    )
    if (
      !seed.turn ||
      !hasAgentSource(seed.turn, {
        conversationId: run.main.id,
        executionId: parent.turn.executionId,
      })
    ) {
      failures.push("the child seed lost its exact agent origin")
      return
    }
    const childText = prompt(run, STRESS.ownedChild, step)
    await run.client.conversation.submit(
      run.client.conversation.prepare(childIds[1], childText),
    )
    const child = await untilTurn(
      run,
      childIds[1],
      { text: childText },
      (turn) => turn.status === "running" && owned(turn),
    )
    const grandchildIds = owned(child.turn)
    if (
      !grandchildIds ||
      child.turn?.status !== "running" ||
      child.turn.agent !== undefined
    ) {
      failures.push("the child did not identify its running owned execution")
      return
    }
    childIds[2] = child.turn.executionId
    run.owned.add(grandchildIds[1])
    const grandchild = await untilTurn(
      run,
      grandchildIds[1],
      0,
      (turn) =>
        turn.executionId === grandchildIds[2] &&
        turn.status === "running" &&
        (turn.parts ?? []).some((part) => part.text?.includes(SAYS.waiting)),
    )
    if (!grandchild.turn || grandchild.turn.status !== "running") {
      failures.push("the grandchild never reached its running waiting turn")
      return
    }
    if (
      !hasAgentSource(grandchild.turn, {
        conversationId: childIds[1],
        executionId: child.turn.executionId,
      })
    ) {
      failures.push("the grandchild lost its exact child-turn author")
      return
    }
    await expectShown(run.page, SAYS.owned, failures)
    if (!(await stopShown(run.page))) {
      failures.push("the parent did not offer Stop while its descendants ran")
      return
    }
    const reportQueued = await waitFor(async () => {
      const view = await run.client.conversation.read(run.main.id)
      return view.messages.some(
        (turn) =>
          hasAgentSource(turn, {
            conversationId: childIds[1],
            executionId: seedExecution,
          }) && turn.status === "queued",
      )
    }, 30_000)
    if (!reportQueued) {
      failures.push("the child's completed seed report was not queued before parent Stop")
      return
    }
    const beforeStop = await seenTurns(run, run.main.id)
    const checkpoint = parentStopCheckpoint(readFileSync(run.acpRecording, "utf8"), text)
    await dock(run.page)
      .getByRole(...composerControls.stop)
      .click({ timeout: 10_000 })
    const stopped = await settled(run, run.main.id, seen, text, "cancelled", failures)
    if (stopped.turn?.executionId !== parent.turn.executionId)
      failures.push("Stop changed the selected parent execution identity")
    for (const [id, executionId] of [childIds.slice(1), grandchildIds.slice(1)]) {
      const final = await waitFor(async () => {
        const view = await run.client.conversation.read(id)
        const turn = view.messages.find((each) => each.executionId === executionId)
        return turn && ended(turn) ? turn : null
      }, 60_000)
      if (final?.status !== "cancelled")
        failures.push(
          `owned execution ${executionId} ended ${final?.status ?? "missing"}`,
        )
    }
    // A terminal status alone can precede queue settlement. Require the full
    // current views to have no pending work or unanswered review/question.
    for (const id of [run.main.id, childIds[1], grandchildIds[1]]) {
      const idle = await waitFor(async () => {
        const view = await readWithReviews(run.client, id)
        return (
          view.queueComplete &&
          view.messages.every(ended) &&
          view.pending.length === 0 &&
          view.permissions.length === 0 &&
          view.questions.length === 0
        )
      }, 60_000)
      if (!idle) failures.push(`${id} did not settle after recursive Stop`)
    }
    if (!(await sendShown(run.page))) failures.push("parent Stop did not return to Send")
    await expectShown(run.page, "Cancelled", failures)
    // Child cancellation reports may retry; none may reopen the stopped parent.
    await sleep(1500)
    const after = await run.client.conversation.read(run.main.id)
    if (
      parentRestarted(
        checkpoint,
        readFileSync(run.acpRecording, "utf8"),
        beforeStop,
        after.messages,
      ) ||
      after.messages.find((turn) => turn.executionId === parent.turn.executionId)
        ?.status !== "cancelled"
    )
      failures.push(
        `a cancellation report restarted the stopped parent: ${JSON.stringify(
          after.messages
            .filter((turn) => !beforeStop.has(turn.executionId))
            .map((turn) => ({
              executionId: turn.executionId,
              status: turn.status,
              parts: turn.parts?.length ?? 0,
              text: turn.userText?.slice(0, 80),
              agent: turn.agent,
            })),
        )}`,
      )
    if (!(await sendShown(run.page)))
      failures.push("a report returned the parent to Stop")
  },

  async restore(run, step, failures) {
    const manifest = run.stack.restore
    const loaded = manifest?.loaded?.[step.slot]
    const missing = manifest?.missing?.[step.slot]
    const source = manifest?.source
    if (!loaded || !missing || !source)
      throw new Error("the stress restore was not prepared")
    if (!existsSync(source))
      failures.push(`the restored source ${source} is not there to load`)
    // Both restored conversations start archived, so followers do not fetch
    // them before this step. Listing the loaded one is setup. The window
    // opens it while the source is still there, and the client reads the
    // result only after that.
    const prepare = await connectClient(run.stack.gateway, INSTANCE, "panel")
    let listed = false
    try {
      const result = await prepare.conversation.unarchive(loaded.id)
      listed = result.applied === true
      if (!listed) failures.push("the restored conversation was already listed")
    } catch (error) {
      failures.push(`the restored conversation could not be listed: ${error.message}`)
    } finally {
      prepare.close()
    }
    if (listed) {
      await openStressConversation(run.page, loaded.id, loaded.title)
      // The list preview is "restored-history". This sentence is only the turn.
      if (!(await shows(run.page, "restored turn text", 15_000))) {
        failures.push(
          `the restored history was not shown: ${JSON.stringify((await pageText(run.page)).slice(0, 500))}`,
        )
      }
      const panel = await connectClient(run.stack.gateway, INSTANCE, "panel")
      try {
        const view = await panel.conversation.read(loaded.id)
        const said = (view.messages ?? [])
          .map((turn) => `${turn.status ?? ""} ${turn.userText ?? ""}`)
          .join("\n")
        if (!said.includes("restored turn text"))
          failures.push(
            `the gateway view did not load the restored history: ${JSON.stringify(said)}`,
          )
      } catch (error) {
        failures.push(`the gateway could not read the restored history: ${error.message}`)
      } finally {
        panel.close()
      }
    }
    const hidden = `${source}.aside`
    renameSync(source, hidden)
    let revealed = false
    try {
      const listing = await connectClient(run.stack.gateway, INSTANCE, "panel")
      try {
        const result = await listing.conversation.unarchive(missing.id)
        revealed = result.applied === true
        if (!revealed)
          failures.push("the conversation whose source is gone was already listed")
      } catch (error) {
        failures.push(
          `the conversation whose source is gone could not be listed: ${error.message}`,
        )
      } finally {
        listing.close()
      }
      if (revealed) {
        await openStressConversation(run.page, missing.id, missing.title)
        const text = prompt(run, STRESS.restore, step, " missing")
        const box = dock(run.page).locator("textarea")
        await box.fill(text)
        await dock(run.page)
          .getByRole(...composerControls.send)
          .click({ timeout: 10_000 })
        await expectShown(run.page, "no longer at its source", failures)
        // The desktop keeps a refused send as the unsent message, which is the
        // draft: the words stay on screen, in the field or beside the refusal.
        const kept = await waitFor(
          async () =>
            (await box.inputValue()) === text ||
            (await pageText(run.page)).includes(text),
          10_000,
        )
        if (!kept) {
          failures.push(
            `a send whose restored source is gone did not keep the draft: ${JSON.stringify(await box.inputValue())} ${JSON.stringify((await pageText(run.page)).slice(0, 500))}`,
          )
        }
      }
    } finally {
      if (revealed) {
        const listing = await connectClient(run.stack.gateway, INSTANCE, "panel")
        try {
          await listing.conversation.archive(missing.id)
        } catch (error) {
          failures.push(
            `the conversation whose source is gone stayed listed: ${error.message}`,
          )
        } finally {
          listing.close()
        }
      }
      if (existsSync(hidden)) renameSync(hidden, source)
    }
    await openStressConversation(run.page, run.main.id, run.main.title)
  },

  async fail(run, step, failures) {
    const text = prompt(run, STRESS.fail, step)
    const seen = await send(run, run.main.id, text, failures)
    await settled(run, run.main.id, seen, text, "failed", failures)
    await expectShown(run.page, SAYS.beforeFailure, failures)
  },

  async usageLimit(run, step, failures) {
    const text = prompt(run, STRESS.limit, step)
    const seen = await send(run, run.main.id, text, failures)
    const settledTurn = await settled(run, run.main.id, seen, text, "failed", failures)
    if (!settledTurn.turn) return
    const limit = settledTurn.turn.usageLimit
    if (!limit || limit.provider !== run.stack.agent || limit.recovery !== "unknown")
      failures.push(
        `the turn is not an unknown ${run.stack.agent} usage limit: ${JSON.stringify(limit)}`,
      )
    if (settledTurn.turn.error)
      failures.push(`a usage limit kept a diagnostic: ${settledTurn.turn.error}`)
    const after = await run.client.conversation.read(run.main.id)
    if (after.messages.some((each) => each.retried && each.userText === text))
      failures.push("a usage limit resumed the turn")
    await expectShown(run.page, "Usage limit reached", failures)
    await expectShown(run.page, "Reset time unknown", failures)
    const box = run.page.getByRole("checkbox", {
      name: "Auto-continue this project's threads when limits reset",
    })
    await box.check()
    if (
      !(await waitFor(async () => {
        const view = await run.client.conversation.read(run.main.id)
        return view.usageLimitAutoContinue === true
      }, 15_000))
    )
      failures.push("the gateway did not turn usage-limit continuation on")
    await box.uncheck()
    if (
      !(await waitFor(async () => {
        const view = await run.client.conversation.read(run.main.id)
        return view.usageLimitAutoContinue === false
      }, 15_000))
    )
      failures.push("the gateway did not turn usage-limit continuation off")
  },

  async signIn(run, step, failures) {
    const text = prompt(run, STRESS.signIn, step)
    const seen = await send(run, run.main.id, text, failures)
    const settledTurn = await settled(run, run.main.id, seen, text, "failed", failures)
    if (!settledTurn.turn) return
    if (settledTurn.turn.authenticationRequired !== true)
      failures.push("the gateway did not publish the refusal as authentication required")
    if (settledTurn.turn.error)
      failures.push(`a sign-in refusal kept a diagnostic: ${settledTurn.turn.error}`)
    const view = await run.client.conversation.read(run.main.id)
    const environment = view.lease?.environment
    const name = run.stack.agent === "codex" ? "Codex" : "Claude"
    // ADR 501: Claude refused in a Boat VM mints a sign-in for Boat; any other
    // run is offered this computer's own sign-in.
    const label =
      run.stack.agent === "claude" && environment === "boat"
        ? "Authenticate"
        : `Sign in to ${name}`
    await expectShown(run.page, "Your login expired", failures)
    const button = run.page.locator(".provider-sign-in button")
    const said = await button
      .first()
      .textContent({ timeout: 10_000 })
      .catch(() => null)
    if (said !== label)
      failures.push(
        `the sign-in card offers "${said}" for a run in ${environment ?? "no lease"}, not "${label}"`,
      )
  },

  async switch(run, step, failures) {
    if (step.during) {
      const text = prompt(run, STRESS.pause, step)
      const seen = await send(run, run.main.id, text, failures)
      if (!(await stopShown(run.page)))
        failures.push("Send did not turn into Stop while the turn ran")
      await pickAccess(run.page, step.to)
      // A change made mid-turn says nothing: the shield shows the choice at
      // once, the gateway takes it while the turn runs (the `raise` step
      // checks the turn's next call is decided at it), and the composer adds
      // no line about it.
      const chosen = await waitFor(
        async () =>
          (await dock(run.page)
            .locator(composerControls.access)
            .getAttribute("data-access-mode")
            .catch(() => null)) === composerControls.accessValue[step.to],
        5_000,
      )
      if (!chosen) failures.push("the shield did not show the choice while the turn ran")
      for (const _ of Array.from({ length: 6 })) {
        await composerSaysNothing(run.page, "of the mid-turn access change", failures)
        await run.page.waitForTimeout(250)
      }
      if (run.evidence && !run.accessHintShot) {
        run.accessHintShot = true
        await dock(run.page)
          .locator(".desktop-composer")
          .screenshot({ path: join(run.evidence, `access-held-${run.engine}.png`) })
          .catch(() => {})
      }
      await settled(run, run.main.id, seen, text, "completed", failures)
    } else {
      await pickAccess(run.page, step.to)
    }
    if (!(await accessIs(run, run.main.id, step.to)))
      failures.push(`the gateway does not run the conversation at ${step.to}`)
    const shield = await dock(run.page)
      .locator(composerControls.access)
      .getAttribute("data-access-mode")
      .catch(() => null)
    if (shield !== composerControls.accessValue[step.to])
      failures.push(
        `the shield shows ${shield}, not ${composerControls.accessValue[step.to]}`,
      )
    run.mode = step.to
  },

  async raise(run, step, failures) {
    if (!(await accessIs(run, run.main.id, step.from)))
      failures.push(`the raise does not start at ${step.from}`)
    const recordingBefore = readFileSync(run.stack.acpRecording, "utf8")
    const text = prompt(run, STRESS.raise, step)
    const seen = await send(run, run.main.id, text, failures)
    const first = `Raise 1 of ${RAISE_REVIEWS}`
    const asked = await untilTurn(
      run,
      run.main.id,
      { text },
      (_turn, current) => current.permissions.some((each) => each.title === first),
      30,
    )
    if (!asked.view?.permissions?.some((each) => each.title === first)) {
      failures.push("the raise turn did not ask its first review")
      return
    }
    if (!(await card(run.page)))
      failures.push("no card for the review open when access was raised")
    // Raised while the turn waits on that card, as a person does who meant
    // to give it full access and is asked anyway.
    await pickAccess(run.page, step.to)
    if (!(await accessIs(run, run.main.id, step.to)))
      failures.push(`the gateway did not run the chat at ${step.to} after the raise`)
    // The review open at the raise is allowed by it; none after it is asked,
    // so the turn ends with nobody clicking anything.
    const { turn, view } = await settled(
      run,
      run.main.id,
      seen,
      text,
      "completed",
      failures,
    )
    const said = (turn?.parts ?? []).map((part) => part.text ?? "").join("")
    for (let n = 1; n <= RAISE_REVIEWS; n += 1)
      if (!said.includes(`Raised ${n}.`))
        failures.push(
          `review ${n} of the raise turn was not allowed: ${JSON.stringify(said.slice(-120))}`,
        )
    if (!said.includes(SAYS.raised)) failures.push("the raise turn did not finish")
    // On the agent's wire: the mode change went while the turn waited on its
    // first review, that review was answered after it, and nothing more was
    // asked (the turn runs fast once allowed, so a read of the view cannot
    // tell this apart from a change held until the turn ended).
    const wire = raisedTurn(
      recordingBefore,
      readFileSync(run.stack.acpRecording, "utf8"),
      text,
    )
    if (!wire) failures.push("the raise turn's prompt is not in the agent's recording")
    else {
      if (!wire.modeChanged)
        failures.push(
          "the raise reached the agent only after its turn, not while it waited",
        )
      if (!wire.answeredAfterChange)
        failures.push(
          "the review open at the raise was answered before the agent took it",
        )
      if (wire.asked !== 1)
        failures.push(
          `the raised turn asked ${wire.asked} reviews, not only the one open at the raise`,
        )
    }
    if (view?.permissions?.length)
      failures.push(`${view.permissions.length} reviews still open after the raise`)
    if (await card(run.page, 1_000))
      failures.push("an approval card is still shown after the raise")
    await expectShown(run.page, SAYS.raised, failures)
    const shield = await dock(run.page)
      .locator(composerControls.access)
      .getAttribute("data-access-mode")
      .catch(() => null)
    if (shield !== composerControls.accessValue[step.to])
      failures.push(`the shield shows ${shield} after the raise, not ${step.to}`)
    run.mode = step.to
  },

  async "refused-access"(run, step, failures) {
    let text, seen
    if (step.during) {
      text = prompt(run, STRESS.pause, step)
      seen = await send(run, run.main.id, text, failures)
      if (!(await stopShown(run.page)))
        failures.push("Send did not turn into Stop before the refused access choice")
    }
    const before = await readWithReviews(run.client, run.main.id)
    requireStressCapabilities(run.stack.profile, {
      modelId: before.runtime?.model,
      approvalModes: before.approvalModes,
    })
    const recording = readFileSync(run.stack.acpRecording, "utf8")
    const observation = run.accessRefusals.begin({
      conversationId: run.main.id,
      mode: "full",
    })
    let refused = false
    try {
      await pickAccess(run.page, "full")
      if (step.during) await settled(run, run.main.id, seen, text, "completed", failures)
      refused = await waitFor(() => observation.refused(), 20_000)
    } finally {
      const evidence = observation.finish()
      run.accessRefusalEvidence.push(evidence)
      if (run.evidence)
        writeView(
          run.evidence,
          `views/${run.engine}-access-refusals.json`,
          run.accessRefusalEvidence,
        )
      if (
        !refused ||
        evidence.outcome !== "refused" ||
        evidence.code !== "approval_mode_unavailable"
      )
        failures.push(
          `Full was not explicitly refused as unavailable: ${evidence.outcome}/${evidence.code}`,
        )
    }
    const notice = await waitFor(
      async () =>
        (
          (await dock(run.page)
            .locator(composerControls.access)
            .getAttribute("data-tooltip")
            .catch(() => "")) ?? ""
        ).includes("This model doesn’t offer that access."),
      10_000,
    )
    if (!notice) failures.push("the shield did not explain the unavailable access choice")
    await composerSaysNothing(run.page, "of the unavailable access choice", failures)
    const after = await readWithReviews(run.client, run.main.id)
    if (
      before.approvalMode !== "ask" ||
      after.approvalMode !== "ask" ||
      after.runtime?.model !== before.runtime?.model ||
      JSON.stringify(after.approvalModeChange) !==
        JSON.stringify(before.approvalModeChange) ||
      JSON.stringify(after.messages.map((turn) => turn.executionId)) !==
        JSON.stringify(before.messages.map((turn) => turn.executionId))
    )
      failures.push(
        "the refused Full choice changed the selection, pending change or admitted turns",
      )
    if (
      (await dock(run.page)
        .locator(composerControls.access)
        .getAttribute("data-access-mode")) !== composerControls.accessValue.ask
    )
      failures.push("the shield did not retain Ask after the refusal")
    if (elevatedAccessAfter(recording, readFileSync(run.stack.acpRecording, "utf8")))
      failures.push("the refused Full choice reached the provider as elevated access")
  },

  async restart(run, step, failures) {
    const text = prompt(run, STRESS.cut, step)
    await run.exclusive(async () => {
      const seen = await send(run, run.main.id, text, failures)
      await expectShown(run.page, SAYS.pausing, failures)
      // It may wait behind a turn of the agent's own, such as a thread's report.
      const { turn, view } = await untilTurn(
        run,
        run.main.id,
        { text },
        (current) => current.status === "running",
        60,
      )
      if (turn?.status !== "running") {
        failures.push(
          `the turn to cut off is ${turn?.status ?? "missing"}, not running: ${JSON.stringify(
            {
              messages: view?.messages
                .filter((each) => !seen.has(each.executionId))
                .map((each) => [each.status, each.userText.slice(0, 60)]),
              pending: view?.pending?.length,
            },
          )}`,
        )
        return
      }
      await run.restartConsole.during(run.opened, async () => {
        const before = failures.length
        const old = run.client
        run.restarts += 1
        await run.stack.gateway.restart(step.signal)
        old.close()
        run.client = await connectClient(run.stack.gateway, INSTANCE, "panel")
        // The cut-off turn is carried on by a successor: a notice, then the same text.
        const resumed = await waitFor(async () => {
          const view = await run.client.conversation.read(run.main.id)
          return view.messages.find(
            (each) =>
              !seen.has(each.executionId) &&
              each.retried === true &&
              each.userText === text &&
              each.status === "completed",
          )
        }, 90_000)
        if (!resumed) {
          const view = await run.client.conversation.read(run.main.id).catch(() => null)
          failures.push(
            `the cut-off turn was not resumed after ${step.signal}: ${JSON.stringify(
              view?.messages
                .filter((each) => !seen.has(each.executionId))
                .map((each) => [each.status, each.userText.slice(0, 60)]),
            )}`,
          )
          return
        }
        // The window shows the person's own words, marked Retried, and never
        // the notice written for the agent as theirs (UI audit #2).
        const resumedInWindow = await waitFor(async () => {
          const text = await pageText(run.page)
          return (
            text.includes(fold(resumed.userText)) &&
            text.includes(fold(SAYS.resumed)) &&
            (await run.page.locator('[data-role="user"] [data-retried]').count()) > 0
          )
        }, 60_000)
        const notices = await run.page
          .locator('[data-role="user"] .workspace-bubble')
          .allInnerTexts()
        if (
          notices.some((said) =>
            /^\[(Nessa restarted|Your last turn|This message did not reach)/.test(said),
          )
        )
          failures.push(
            "the window shows the agent's resume notice as the person's message",
          )
        if (!resumedInWindow)
          failures.push(
            "the window does not show the current resumed turn and its completion",
          )
        return failures.length === before
      })
    })
  },

  async drift(run, step, failures) {
    await run.exclusive(async () => {
      await run.restartConsole.during(run.opened, async () => {
        const before = failures.length
        // The composer is already on screen. The window has come back only
        // once its new session socket has received a frame.
        let stop = () => {}
        const recovered = new Promise((resolve) => {
          const onSocket = (socket) => {
            if (!socket.url().endsWith("/session")) return
            const received = () => resolve(true)
            stop = () => {
              socket.off("framereceived", received)
              run.page.off("websocket", onSocket)
            }
            socket.once("framereceived", received)
          }
          run.page.on("websocket", onSocket)
          stop = () => run.page.off("websocket", onSocket)
        })
        const old = run.client
        run.restarts += 1
        const drifted = mkdtempSync(join(tmpdir(), "nessa-stress-drift-"))
        const path = `/opt/homebrew/bin:${run.stack.gatewayPath}`
        try {
          await run.stack.gateway.restart("SIGTERM", {
            overlay: {
              NESSA_AGENT_PATH: path,
              PATH: path,
              TMPDIR: drifted,
            },
            whileStopped: async () => moveStagedRuntime(run.stack.gateway.directory),
          })
          old.close()
          run.client = await connectClient(run.stack.gateway, INSTANCE, "panel")
          const back = await Promise.race([recovered, sleep(60_000).then(() => false)])
          if (!back)
            failures.push("the window did not come back after the agent path changed")
        } finally {
          stop()
        }
        return failures.length === before
      })
    })
    // The existing conversation still takes a message, in the window.
    if (failures.length === 0) await steps.reply(run, step, failures)
    if (failures.length === 0) await unusedNativeContext(run, step, failures)
  },

  async linking(run, step, failures) {
    await run.exclusive(async () => {
      await openLinkedDevices(run.page)
      if (!(await switchLinking(run, true, failures))) return
      if (!(await announced(run, failures))) return
      if (!(await pairFromWindow(run, failures))) return
      // Away and back: the tab reads linking again, and it is still on.
      await closeSettings(run.page)
      await openLinkedDevices(run.page)
      if (!(await switchLinking(run, false, failures))) return
      await closeSettings(run.page)
    })
    // The restarted gateway still answers the conversation, in the window.
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async passphrase(run, step, failures) {
    await run.exclusive(async () => {
      const needsUnlock = (await run.client.vault.status()).state === "locked"
      await openAccounts(run.page)
      if (needsUnlock) await run.page.locator('[data-vault-dialog="unlock"]').waitFor()
      await unlockVault(run.page, { expectedContainer: "[data-settings-layers]" })
      const locked = async () => (await run.client.vault.status()).state === "locked"
      await changeVaultPassphrase(
        run.page,
        `disposable cobalt orchard vault stress passphrase ${step.index}`,
        { locked },
      )
      // Keep the shared disposable fixture phrase for restart/other steps.
      await changeVaultPassphrase(run.page, VAULT_PASSPHRASE, { locked })
      if ((await run.client.vault.status()).state !== "unlocked")
        failures.push(
          "Explicit unlock after passphrase change was not reflected by gateway",
        )
      await closeSettings(run.page)
    })
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async touchId(run, step, failures) {
    const touchId = run.stack.vault.touchId
    const dialog = run.page.locator('[data-vault-dialog="unlock"]')
    const remember = dialog.getByLabel("Use Touch ID to unlock next time", {
      exact: true,
    })
    const useTouchId = dialog.getByRole("button", { name: "Use Touch ID", exact: true })
    const state = async () => (await run.client.vault.status()).state
    // Sealed, then Accounts opened: its rows ask for the vault, as a launch does.
    const leaveSettings = async () => {
      if (await run.page.$(css.settings)) await closeSettings(run.page)
    }
    const lockedAccounts = async () => {
      await leaveSettings()
      // A gateway restarted by an earlier step ends the app's vault session;
      // a status read reconnects it, as the app does, before the lock.
      await run.stack.vault.command({ operation: "status" })
      const lock = await run.stack.vault.command({ operation: "lock" })
      if (!lock.ok) throw new Error(`the vault would not lock: ${JSON.stringify(lock)}`)
      await openAccounts(run.page)
    }
    await run.exclusive(async () => {
      touchId.available = true
      touchId.saved = null
      touchId.answer = "pass"
      touchId.prompts = 0
      try {
        // 1. Offered, ticked, on the typed unlock; the accepted passphrase is kept.
        await lockedAccounts()
        await dialog.waitFor()
        if (!(await remember.isChecked()))
          failures.push("the unlock dialog did not offer Touch ID ticked")
        if (run.evidence)
          await run.page
            .getByRole("dialog", { name: "Unlock Nessa", exact: true })
            .screenshot({ path: join(run.evidence, `touch-id-offer-${run.engine}.png`) })
            .catch(() => {})
        await dialog.getByLabel("Passphrase", { exact: true }).fill(VAULT_PASSPHRASE)
        await dialog.getByRole("button", { name: "Unlock", exact: true }).click()
        await dialog.waitFor({ state: "hidden" })
        if (touchId.saved !== VAULT_PASSPHRASE)
          failures.push("the passphrase was not kept for Touch ID after a ticked unlock")
        const toggle = run.page
          .locator(selectorFor.settingRow("vault-touch-id"))
          .getByRole("switch")
        await toggle.waitFor()
        if ((await toggle.getAttribute("aria-checked")) !== "true")
          failures.push("Settings does not show Touch ID on after it was kept")

        // 2. The next unlock is a fingerprint alone: nothing typed.
        await lockedAccounts()
        for (const until = Date.now() + 15_000; Date.now() < until;) {
          if ((await state()) === "unlocked" && !(await dialog.isVisible())) break
          await run.page.waitForTimeout(100)
        }
        if ((await state()) !== "unlocked")
          failures.push("a passing fingerprint did not unlock the vault")
        if (await dialog.isVisible())
          failures.push("the unlock dialog stayed open after a passing fingerprint")
        if (touchId.prompts !== 1)
          failures.push(`the fingerprint was asked ${touchId.prompts} times, not once`)

        // 3. A dismissed prompt leaves the form, and its button asks again.
        touchId.answer = "cancel"
        await lockedAccounts()
        // The button shows, resting, while the prompt is up; it wakes after.
        await useTouchId.waitFor()
        await run.page
          .waitForFunction(
            (button) => button && !button.disabled,
            await useTouchId.elementHandle(),
            { timeout: 10_000 },
          )
          .catch(() =>
            failures.push("after a dismissed prompt, Use Touch ID rests disabled"),
          )
        if ((await dialog.getByRole("alert").count()) !== 0)
          failures.push("a dismissed Touch ID prompt was shown as a failure")
        if (touchId.prompts !== 2)
          failures.push("a dismissed prompt was asked again on its own")
        touchId.answer = "pass"
        await useTouchId.click()
        await dialog.waitFor({ state: "hidden" })
        if ((await state()) !== "unlocked")
          failures.push("Use Touch ID after a dismissed prompt did not unlock")

        // 4. Changed fingerprints drop the copy and say so; typing keeps it again.
        touchId.answer = "changed"
        await lockedAccounts()
        await dialog
          .getByRole("alert")
          .filter({ hasText: "Your fingerprints changed" })
          .waitFor()
        if (touchId.saved !== null)
          failures.push("changed fingerprints left the passphrase kept")
        await remember.waitFor()
        await dialog.getByLabel("Passphrase", { exact: true }).fill(VAULT_PASSPHRASE)
        await dialog.getByRole("button", { name: "Unlock", exact: true }).click()
        await dialog.waitFor({ state: "hidden" })
        if (touchId.saved !== VAULT_PASSPHRASE)
          failures.push("unlocking again after changed fingerprints did not keep it")

        // 5. Settings turns it off, and the switch then rests.
        if ((await toggle.getAttribute("aria-checked")) !== "true")
          failures.push("Settings did not read Touch ID on again after the dialog")
        await toggle.click()
        await run.page
          .locator(
            `${selectorFor.settingRow("vault-touch-id")} [role="switch"][aria-checked="false"]`,
          )
          .waitFor()
        if (touchId.saved !== null)
          failures.push("turning Touch ID off kept the passphrase")
        if (!(await toggle.isDisabled()))
          failures.push(
            "with nothing kept, the Touch ID switch can be turned on from Settings",
          )
        if (run.evidence)
          await run.page
            .locator(selectorFor.settingRow("vault-touch-id"))
            .screenshot({
              path: join(run.evidence, `touch-id-settings-${run.engine}.png`),
            })
            .catch(() => {})
      } finally {
        touchId.available = false
        touchId.saved = null
        touchId.answer = "pass"
        if ((await state()) !== "unlocked") await run.stack.vault.unlock()
        // A dialog left open by a failure above is dismissed first.
        if (await dialog.isVisible()) await run.page.keyboard.press("Escape")
        await leaveSettings()
      }
    })
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async mcp(run, step, failures) {
    const name = `stress-mcp-r${step.index}-s${run.knobs.seed}`
    const variable = "STRESS_TOKEN"
    // Generated, never printed, and looked for in the page and config.json.
    const value = randomUUID()
    const config = join(
      run.stack.gateway.directory,
      "ci",
      "instances",
      INSTANCE,
      "config.json",
    )
    const stored = () =>
      (JSON.parse(readFileSync(config, "utf8")).agents?.mcpServers ?? []).find(
        (server) => server.name === name,
      )
    const valueOf = (server) => server?.env?.[variable]
    const page = run.page
    const row = () => page.locator(selectorFor.mcpRowNamed(name))
    const shot = (what) =>
      run.evidence
        ? page.screenshot({ path: join(run.evidence, `mcp-${what}.png`) }).catch(() => {})
        : undefined
    await run.exclusive(async () => {
      if ((await openMcpServers(page)).prompted)
        failures.push("opening Connections asked for the passphrase")
      await noHistoryButtons(page, failures, "Settings")
      await shot("list")
      // Added from the window, off for new conversations so no agent starts it.
      await page.getByRole("button", { name: names.mcp.add, exact: true }).click()
      const form = page.locator(css.mcpForm)
      await form.getByLabel(names.mcp.name, { exact: true }).fill(name)
      await form.getByLabel(names.mcp.command, { exact: true }).fill(process.execPath)
      await form.getByRole("switch", { name: names.mcp.useInNew }).click()
      await form.getByRole("button", { name: names.mcp.addVariable, exact: true }).click()
      const added = form.locator(css.mcpVariableAdded).last()
      await added.getByLabel(names.mcp.variableName, { exact: true }).fill(variable)
      await added.locator(css.mcpSecret).fill(value)
      await form.getByRole("button", { name: names.mcp.save, exact: true }).click()
      const shown = await row()
        .waitFor({ state: "visible", timeout: 10_000 })
        .then(() => true)
        .catch(() => false)
      if (!shown) {
        failures.push(`${name} was saved but no row came back for it`)
        // Left as the next step expects: no dialog, no Settings.
        if (await form.isVisible().catch(() => false)) await page.keyboard.press("Escape")
        await closeSettings(page).catch(() => {})
        return
      }
      // A closed row: its name, one short line, a switch and its ⋯ menu.
      const line = await row().locator('[data-slot="settings-row-detail"]').innerText()
      if (line !== "Local · 1 variable")
        failures.push(
          `${name}'s row says ${JSON.stringify(line)}, not "Local · 1 variable"`,
        )
      if ((await row().locator(css.mcpSwitch).getAttribute("aria-checked")) !== "false")
        failures.push(`${name}'s switch is on, though it was saved off`)
      if (valueOf(stored()) !== value)
        failures.push(`config.json does not hold ${variable} as typed after the add`)
      if ((await page.content()).includes(value))
        failures.push(`${variable}'s value is in the page after the save`)
      // Edited: no command to change, the stored value at rest, Replace
      // opened and left empty, saved. The value stays as stored.
      await mcpMenu(page, name, names.mcp.edit)
      await shot("edit")
      if ((await form.locator(selectorFor.mcpField("command")).count()) > 0)
        failures.push("the edit form offers the command as a field")
      const command = await form
        .locator(css.mcpCommand)
        .innerText()
        .catch(() => "")
      if (command !== basename(process.execPath))
        failures.push(`the edit form names the command ${JSON.stringify(command)}`)
      const kept = form.locator(selectorFor.mcpVariableNamed(variable))
      if ((await kept.locator(css.mcpSecretStored).count()) !== 1)
        failures.push(`${variable} is not shown as stored in the edit form`)
      await kept.getByRole("button", { name: names.mcp.replaceValue(variable) }).click()
      await kept.locator(css.mcpSecret).fill("")
      await form.getByRole("button", { name: names.mcp.save, exact: true }).click()
      await page.waitForSelector(css.mcpForm, { state: "detached", timeout: 10_000 })
      if (valueOf(stored()) !== value)
        failures.push(
          `${variable}'s stored value changed on a save that did not type one`,
        )
      // Removed, once asked in its dialog.
      await mcpMenu(page, name, names.mcp.remove)
      const ask = page.locator(selectorFor.mcpAction("confirm"))
      await shot("remove")
      await ask.click()
      await row()
        .waitFor({ state: "detached", timeout: 10_000 })
        .catch(() => {})
      if ((await row().count()) > 0)
        failures.push(`${name}'s row is still there after Remove`)
      if (stored()) failures.push(`config.json still holds ${name} after Remove`)
      await closeSettings(page)
    })
    // The gateway still answers the conversation, in the window.
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async github(run, step, failures) {
    const github = run.stack.github
    if (!github) {
      failures.push("the stack was started without the fake GitHub")
      return
    }
    const namespace = join(run.stack.gateway.directory, "ci", "instances", INSTANCE)
    const signIn = join(namespace, "github", "sign-in.json")
    const row = run.page.locator(css.github)
    const state = () => row.getAttribute("data-github-state").catch(() => null)
    // Each state the person sees, kept with the evidence.
    const shot = (name) =>
      run.evidence
        ? run.page
            .screenshot({ path: join(run.evidence, `github-${name}.png`) })
            .catch(() => {})
        : undefined
    // GitHub's avatar host, answered here: which pictures were asked for.
    const avatars = []
    const pictures = `${GITHUB_AVATARS}/**`
    const answer = (route) => {
      avatars.push(route.request().url())
      return route.fulfill({ contentType: "image/png", body: FAKE_AVATAR })
    }
    await run.page.route(pictures, answer)
    // Answered until the step ends: its Disconnect reopens the signed-in row.
    try {
      await run.exclusive(async () => {
        const needsUnlock = (await run.client.vault.status()).state === "locked"
        await openAccounts(run.page)
        if (needsUnlock) await run.page.locator('[data-vault-dialog="unlock"]').waitFor()
        await unlockVault(run.page, {
          screenshot: run.evidence
            ? join(run.evidence, `vault-unlock-settings-${run.engine}.png`)
            : undefined,
          expectedContainer: "[data-settings-layers]",
        })
        if (!(await waitFor(async () => (await state()) === "signed-out", 30_000))) {
          failures.push(`GitHub was ${await state()} before Connect`)
          return
        }
        await shot("signed-out")
        await row.getByRole("button", { name: names.githubConnect }).click()
        await unlockVault(run.page)
        const code = row.locator("[data-github-code]")
        const shown = await waitFor(
          async () =>
            github.userCode() !== null &&
            (await code.getAttribute("data-github-code").catch(() => null)) ===
              github.userCode(),
          30_000,
        )
        if (!shown) {
          failures.push(
            `the row did not show the code GitHub handed out (${github.userCode()})`,
          )
          return
        }
        await shot("connecting")
        const open = await row.locator("[data-github-open]").getAttribute("href")
        if (open !== github.verificationUri)
          failures.push(`Open GitHub goes to ${open}, not ${github.verificationUri}`)
        // The person enters the code on GitHub: the gateway's next poll takes it.
        github.approve()
        const connected = await waitFor(
          async () =>
            (await state()) === "connected" &&
            (await row.textContent()).includes(`@${FAKE_ACCOUNT.login}`) &&
            (await row.locator(css.githubStatus).textContent()) === "Connected",
          30_000,
        )
        if (!connected) {
          failures.push(`the row is ${await state()} after GitHub approved the code`)
          return
        }
        // The picture is GitHub's, asked for by the account's id: the page was
        // answered above, so it loads here with nothing leaving the machine.
        const avatar = run.page.locator(`${css.githubAvatar} img`)
        const pictured = await waitFor(
          () =>
            avatar
              .evaluate((image) => image.complete && image.naturalWidth > 0)
              .catch(() => false),
          10_000,
        )
        if (!pictured) failures.push("the row did not draw the account's picture")
        run.githubAvatarRequested ||= avatars.some((url) =>
          url.includes(`/u/${FAKE_ACCOUNT.id}?`),
        )
        if (!run.githubAvatarRequested)
          failures.push(
            `the picture was not asked for by id: ${avatars.join(", ") || "none"}`,
          )
        // At rest the actions are out of sight; the pointer or focus brings them.
        const opacity = (selector) =>
          row.locator(selector).evaluate((node) => getComputedStyle(node).opacity)
        const shows = (actions) =>
          waitFor(
            async () =>
              (await opacity(css.githubActions)) === (actions ? "1" : "0") &&
              (await opacity(css.githubStatus)) === (actions ? "0" : "1"),
            5_000,
          )
        await run.page.mouse.move(1, 1)
        if (!(await shows(false))) failures.push("the actions show with the row at rest")
        // Hidden is out of reach too: what is under Disconnect's spot at rest is not it.
        const hit = await row
          .getByRole("button", { name: names.githubDisconnect })
          .evaluate((node) => {
            const box = node.getBoundingClientRect()
            const under = document.elementFromPoint(
              box.x + box.width / 2,
              box.y + box.height / 2,
            )
            return Boolean(under?.closest(".settings-github-actions"))
          })
        if (hit) failures.push("the hidden actions take the pointer at rest")
        await shot("connected")
        await row.hover()
        if (!(await shows(true)))
          failures.push("the actions did not show under the pointer")
        await shot("connected-hover")
        await run.page.mouse.move(1, 1)
        if (!(await shows(false)))
          failures.push("the actions still show once the pointer leaves")
        // Both actions are native tab stops. WebKit on macOS walks plain Tab
        // through fields only; Option+Tab (Alt+Tab here) walks every control.
        // The backward chord reaches Choose repositories, and the forward
        // chord returns. Focus in the row is what shows the actions.
        const disconnect = row.getByRole("button", { name: names.githubDisconnect })
        const focused = (locator) =>
          locator.evaluate((node) => node === document.activeElement)
        await disconnect.focus()
        await run.page.keyboard.press(keys.previousControl[run.engine])
        if (!(await focused(row.locator("[data-github-repositories]"))))
          failures.push(
            "backward control navigation from Disconnect did not reach Choose repositories",
          )
        await run.page.keyboard.press(keys.nextControl[run.engine])
        if (!(await focused(disconnect)))
          failures.push(
            "forward control navigation from Choose repositories did not reach Disconnect",
          )
        if (!(await shows(true)))
          failures.push("the actions did not show with focus in the row")
        await shot("connected-focus")
        const repositories = await row
          .locator("[data-github-repositories]")
          .getAttribute("href")
        if (repositories !== github.installUrl)
          failures.push(
            `Choose repositories goes to ${repositories}, not ${github.installUrl}`,
          )
        await closeSettings(run.page)
      })
      if (failures.length) return
      if (existsSync(signIn)) failures.push("GitHub sign-in leaked into a plaintext file")
      // A preceding restart replaces run.client; stack.client still names the
      // original closed session. Read through this run's current connection.
      const persisted = await run.client.github.status()
      if (persisted.state !== "connected")
        failures.push("the OS-held sign-in is unavailable")
      if (run.stack.gateway.log().includes(github.token))
        failures.push("the gateway logged the GitHub token")
      await githubAgent(run, step, " connected", "token=none git=none", failures)
      await githubAgent(run, step, " connected", "token=none git=none", failures, true)
      const priorRepositoryReads = github.calls.repository
      await githubReadAgent(run, step, failures)
      if (github.calls.repository !== priorRepositoryReads + 1)
        failures.push(
          `the broker made ${github.calls.repository - priorRepositoryReads} repository reads, not one`,
        )
      if (failures.length) return
      await run.exclusive(async () => {
        await openAccounts(run.page)
        // The actions are out of reach until the pointer is over the row, as a person's is.
        await row.hover()
        await row.getByRole("button", { name: names.githubDisconnect }).click()
        if (!(await waitFor(async () => (await state()) === "signed-out", 30_000)))
          failures.push(`the row is ${await state()} after Disconnect`)
        await closeSettings(run.page)
      })
      if (failures.length) return
      if (existsSync(signIn)) failures.push("Disconnect left the sign-in file")
      await githubAgent(run, step, " disconnected", "token=none git=none", failures)
      await githubAgent(run, step, " disconnected", "token=none git=none", failures, true)
      // Each step of both is on record, and no record holds the token.
      const audit = join(namespace, "conversations", "audit", "github")
      const records = existsSync(audit) ? readdirSync(audit) : []
      for (const event of [
        "connect-requested",
        "connected",
        "disconnect-requested",
        "disconnected",
      ])
        if (!records.some((name) => name.endsWith(`-${event}.json`)))
          failures.push(`the audit has no ${event} record`)
      if (
        records.some((name) =>
          readFileSync(join(audit, name), "utf8").includes(github.token),
        )
      )
        failures.push("an audit record holds the GitHub token")
    } finally {
      await run.page.unroute(pictures, answer)
    }
  },

  async login(run, step, failures) {
    const agent = run.stack.agent
    const login = `sk-stress-login-r${step.index}-s${run.knobs.seed}`
    const shot = (name) =>
      run.evidence
        ? run.page
            .screenshot({ path: join(run.evidence, `login-${name}.png`) })
            .catch(() => {})
        : undefined
    const row = run.page.locator(`[data-saved-login="${agent}"]`)
    // The login, once saved, is removed again whatever happens after.
    try {
      await steps.loginOnWorker(run, step, failures, { agent, login, row, shot })
    } finally {
      try {
        await run.stack.vault.unlock()
        await run.client.logins.remove(agent)
      } catch (error) {
        failures.push(`the saved login could not be removed again: ${error.message}`)
      }
    }
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async loginOnWorker(run, step, failures, { agent, login, row, shot }) {
    const variable = LOGIN_VARIABLES[agent]
    const namespace = join(run.stack.gateway.directory, "ci", "instances", INSTANCE)
    // Saved in Settings, as a person pastes it.
    await run.exclusive(async () => {
      const needsUnlock = (await run.client.vault.status()).state === "locked"
      await openAccounts(run.page)
      if (needsUnlock) await run.page.locator('[data-vault-dialog="unlock"]').waitFor()
      await unlockVault(run.page, { expectedContainer: "[data-settings-layers]" })
      await row.locator(`[data-saved-login-input="${agent}"]`).fill(login)
      await row.locator(`[data-saved-login-save="${agent}"]`).click()
      await unlockVault(run.page)
      const saved = await row
        .locator('[data-saved-login-state="saved"]')
        .waitFor({ timeout: 15_000 })
        .then(() => true)
        .catch(() => false)
      if (!saved) failures.push("the saved-login row does not say the login is saved")
      else await shot("saved")
      if ((await row.locator(`[data-saved-login-input="${agent}"]`).inputValue()) !== "")
        failures.push("the saved-login row kept the login in its field after saving")
      if ((await run.page.locator("[data-settings-layers]").innerText()).includes(login))
        failures.push("Settings shows the saved login")
      await closeSettings(run.page)
    })
    if (failures.length) return
    const status = await run.client.logins.status()
    if (!status.logins.find((each) => each.agent === agent)?.saved)
      failures.push(`the gateway does not report ${agent}'s login saved`)
    if (JSON.stringify(status).includes(login))
      failures.push("logins.status answered with the login")
    // A worker whose owner lets it take logins, each launch of the agent
    // there recording what its login variable held, one line each.
    const linked = await run.exclusive(async () => {
      await openLinkedDevices(run.page)
      const on = await switchLinking(run, true, failures)
      if (on) await closeSettings(run.page)
      return on
    })
    if (!linked) return
    // Linking restarts the gateway, which locks its vault: unlocked again,
    // so the switched-off worker is refused by the policy, not the lock.
    await run.stack.vault.unlock()
    const launches = join(tmpdir(), `nessa-stress-logins-${randomUUID()}.txt`)
    writeFileSync(launches, "")
    const seen = () => readFileSync(launches, "utf8").split("\n").slice(0, -1)
    let worker
    let owner
    try {
      owner = await connectClient(run.stack.gateway, INSTANCE, "owner")
      const launch = scriptedLaunch(agent, {
        scenario: run.stack.scenario,
        nativeContext: run.stack.nativeContext,
        model: run.stack.profile.model,
      })
      const recording = [
        "/bin/sh",
        "-c",
        `printf '%s\\n' "\${${variable}-}" >> '${launches}'; exec "$@"`,
        "sh",
        ...launch.command.argv,
      ]
      worker = await startWorker({
        owner,
        address: run.nativeAddress,
        agent,
        launch: { ...launch, command: { ...launch.command, argv: recording } },
        label: `login${step.index}`,
        logins: true,
        connected: async (name) =>
          (await offers(run, (environments) => environments.includes(name))).includes(
            name,
          ),
      })
      const talk = async (label) => {
        const before = seen().length
        const conversationId = randomUUID()
        await run.client.conversation.create({
          conversationId,
          agent,
          environment: worker.name,
        })
        const text = prompt(run, STRESS.reply, step, ` login ${label}`)
        await run.client.conversation.submit(
          run.client.conversation.prepare(conversationId, text),
        )
        const { view } = await settled(
          run,
          conversationId,
          new Set(),
          text,
          "completed",
          failures,
        )
        if (view?.lease?.host !== worker.name)
          failures.push(`its lease runs on ${view?.lease?.host}, not ${worker.name}`)
        if (failures.length === 0) failures.push(...(await atRest(run, [conversationId])))
        return seen().slice(before)
      }
      // Off: the worker takes logins, but its switch is not on.
      const off = await talk("off")
      if (off.length === 0 || off.some((line) => line !== ""))
        failures.push(
          `with the switch off the agent's launches held ${JSON.stringify(off.map((line) => `${line.length} bytes`))}`,
        )
      if (failures.length) return
      // On, from the worker's own page in Devices.
      const id = worker.name.slice("peer:".length)
      await run.exclusive(async () => {
        const page = run.page
        await setSideRail(page, true, failures)
        if (failures.length) return
        await page.locator('[data-rail-item="devices"]').click()
        const tile = page.locator(`[data-devices-worker="${id}"]`)
        await tile.waitFor({ timeout: 15_000 })
        await tile.click()
        const fact = page.locator('[data-devices-page="worker"] [data-device-logins]')
        await fact.waitFor({ timeout: 5_000 })
        await fact.getByRole("switch").click()
        await unlockVault(page)
        const on = await waitFor(
          async () => (await fact.getAttribute("data-device-logins")) === "on",
          15_000,
        )
        if (!on) failures.push("the worker's saved-logins switch did not turn on")
        else await shot("device-on")
        await page.locator('[data-devices-action="back"]').click()
        await page.locator('[data-rail-item="agents"]').click()
        await setSideRail(page, false, failures)
      })
      if (failures.length) return
      if (!(await run.client.logins.status()).devicesOn.includes(worker.name))
        failures.push("the gateway does not report the worker switched on")
      const on = await talk("on")
      if (on.length === 0 || on.some((line) => line !== login))
        failures.push(
          `with the switch on the agent's launches held ${JSON.stringify(on.map((line) => (line === login ? "the login" : `${line.length} other bytes`)))}`,
        )
      // Withheld and then sent, each recorded, and no record or log holds
      // the login itself.
      const audit = join(namespace, "conversations", "audit", "logins")
      const records = existsSync(audit)
        ? readdirSync(audit).map((name) => readFileSync(join(audit, name), "utf8"))
        : []
      const handed = records
        .map((text) => JSON.parse(text))
        .filter((record) => record.details?.environment === worker.name)
      if (
        !handed.some(
          (record) =>
            record.phase === "withheld" && record.details.reason === "not_allowed",
        )
      )
        failures.push(
          `no record says the policy withheld the login from the switched-off worker: ${JSON.stringify(handed)}`,
        )
      if (!handed.some((record) => record.phase === "sent"))
        failures.push("no record says the login was sent to the switched-on worker")
      if (records.some((text) => text.includes(login)))
        failures.push("a saved-login record holds the login")
      if (run.stack.gateway.log().includes(login) || worker.log().includes(login))
        failures.push("a log holds the saved login")
    } finally {
      if (worker) {
        if (run.evidence)
          writeFileSync(join(run.evidence, `worker-${worker.label}.log`), worker.log())
        await worker.close()
      }
      owner?.close()
      rmSync(launches, { force: true })
      // The switch the step turned on is off again.
      try {
        await run.stack.vault.unlock()
        if (worker?.name) await run.client.logins.setDevice(worker.name, false)
      } catch (error) {
        failures.push(`the worker's switch could not be turned off: ${error.message}`)
      }
      await run.exclusive(async () => {
        await openLinkedDevices(run.page)
        if (await switchLinking(run, false, failures)) await closeSettings(run.page)
      })
    }
  },

  async chatgpt(run, step, failures) {
    const openai = run.stack.openai
    if (!openai) {
      failures.push("the stack was started without the fake OpenAI")
      return
    }
    const [first, second] = FAKE_CHATGPT_ACCOUNTS
    const shot = (name) =>
      run.evidence
        ? run.page
            .screenshot({ path: join(run.evidence, `chatgpt-${name}.png`) })
            .catch(() => {})
        : undefined
    const panel = run.page.locator(".settings-chatgpt")
    const chatgpt = async () => (await run.client.logins.status()).chatgpt
    const idOf = async (account) =>
      (await chatgpt())?.accounts.find((each) => each.email === account.email)?.id
    try {
      // Both accounts signed in from Settings, the browser played by the
      // fake: Continue, then its address opened as the account.
      await run.exclusive(async () => {
        const needsUnlock = (await run.client.vault.status()).state === "locked"
        await openAccounts(run.page)
        if (needsUnlock) await run.page.locator('[data-vault-dialog="unlock"]').waitFor()
        await unlockVault(run.page, { expectedContainer: "[data-settings-layers]" })
        for (const [index, account] of [first, second].entries()) {
          await panel.locator("[data-chatgpt-continue]").click()
          await unlockVault(run.page)
          const open = panel.locator("[data-chatgpt-open]")
          if (
            !(await open
              .waitFor({ timeout: 15_000 })
              .then(() => true)
              .catch(() => false))
          ) {
            failures.push(`no Open ChatGPT link for ${account.email}`)
            return
          }
          if (index === 0) await shot("waiting")
          const back = await openai.approve(await open.getAttribute("href"), account)
          if (back.status !== 200)
            failures.push(`the gateway answered the browser's return with ${back.status}`)
          const ended = await panel
            .locator('[data-chatgpt-ended="signed_in"]')
            .waitFor({ timeout: 15_000 })
            .then(() => true)
            .catch(() => false)
          if (!ended) failures.push(`Settings does not say ${account.email} signed in`)
          const listed = await panel
            .locator(`[data-chatgpt-account="${account.email}"]`)
            .waitFor({ timeout: 5_000 })
            .then(() => true)
            .catch(() => false)
          if (!listed) failures.push(`Settings does not list ${account.email}`)
          if (failures.length) return
        }
        // The first is chosen for Codex.
        const id = await idOf(first)
        const row = panel.locator(`[data-chatgpt-account="${first.email}"]`)
        if ((await row.getAttribute("data-chatgpt-in-use")) === null && id) {
          await row.locator(`[data-chatgpt-use="${id}"]`).click()
          await unlockVault(run.page)
        }
        if (
          !(await waitFor(
            async () => (await chatgpt())?.accounts.find((each) => each.id === id)?.inUse,
            15_000,
          ))
        )
          failures.push(`${first.email} is not in use after Use`)
        else await shot("accounts")
        const shown = await run.page.locator("[data-settings-layers]").innerText()
        if (openai.secrets().some((secret) => shown.includes(secret)))
          failures.push("Settings shows a ChatGPT token")
        await closeSettings(run.page)
      })
      if (failures.length) return
      const status = await run.client.logins.status()
      if (status.chatgpt?.accounts.length !== 2)
        failures.push(
          `the gateway keeps ${status.chatgpt?.accounts.length} ChatGPT accounts, not 2`,
        )
      if (openai.secrets().some((secret) => JSON.stringify(status).includes(secret)))
        failures.push("logins.status answered with a ChatGPT token")
      if (failures.length) return
      await steps.chatgptOnWorker(run, step, failures, { first, second, idOf })
    } finally {
      // Both removed again whatever happened, each revoked at OpenAI.
      try {
        await run.stack.vault.unlock()
        for (const each of (await chatgpt())?.accounts ?? [])
          await run.client.logins.chatgptRemove(each.id)
      } catch (error) {
        failures.push(`the ChatGPT accounts could not be removed again: ${error.message}`)
      }
    }
    if ((await chatgpt())?.accounts.length)
      failures.push("a ChatGPT account was kept after removal")
    if (openai.calls.revoke < 2)
      failures.push(
        `removing both accounts revoked ${openai.calls.revoke} refresh tokens, not 2`,
      )
    if (openai.calls.reused)
      failures.push(
        `the gateway reused a rotated refresh token ${openai.calls.reused} times`,
      )
    // No record or log holds a token.
    const namespace = join(run.stack.gateway.directory, "ci", "instances", INSTANCE)
    const audit = join(namespace, "conversations", "audit", "logins")
    const records = existsSync(audit)
      ? readdirSync(audit).map((name) => readFileSync(join(audit, name), "utf8"))
      : []
    const secrets = openai.secrets()
    if (records.some((text) => secrets.some((secret) => text.includes(secret))))
      failures.push("a saved-login record holds a ChatGPT token")
    if (secrets.some((secret) => run.stack.gateway.log().includes(secret)))
      failures.push("the gateway's log holds a ChatGPT token")
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async chatgptOnWorker(run, step, failures, { first, second, idOf }) {
    const openai = run.stack.openai
    const agent = "codex"
    const linked = await run.exclusive(async () => {
      await openLinkedDevices(run.page)
      const on = await switchLinking(run, true, failures)
      if (on) await closeSettings(run.page)
      return on
    })
    if (!linked) return
    // Linking restarts the gateway, which locks its vault.
    await run.stack.vault.unlock()
    // Each launch of Codex on the worker records its token file's name, the
    // token in it, and whether a variable held the token instead.
    const launches = join(tmpdir(), `nessa-stress-chatgpt-${randomUUID()}.txt`)
    writeFileSync(launches, "")
    const seen = () =>
      readFileSync(launches, "utf8")
        .split("\n")
        .slice(0, -1)
        .map((line) => {
          const [file, token, key] = line.split(" ")
          return { file, token, key }
        })
    let worker
    let owner
    let following = null
    try {
      owner = await connectClient(run.stack.gateway, INSTANCE, "owner")
      const launch = scriptedLaunch(agent, {
        scenario: run.stack.scenario,
        nativeContext: run.stack.nativeContext,
        model: run.stack.profile.model,
      })
      const recording = [
        "/bin/sh",
        "-c",
        `printf '%s %s %s\\n' "\${${CHATGPT_TOKEN_FILE}:--}" "$(cat "\${${CHATGPT_TOKEN_FILE}:-/dev/null}" 2>/dev/null || true)" "\${CODEX_API_KEY:+key}" >> '${launches}'; exec "$@"`,
        "sh",
        ...launch.command.argv,
      ]
      worker = await startWorker({
        owner,
        address: run.nativeAddress,
        agent,
        launch: { ...launch, command: { ...launch.command, argv: recording } },
        label: `chatgpt${step.index}`,
        logins: true,
        connected: async (name) =>
          (await offers(run, (environments) => environments.includes(name))).includes(
            name,
          ),
      })
      await run.client.logins.setDevice(worker.name, true)
      // `keep`: followed from the start, so its agent is not parked once
      // idle and its lease stays.
      const talk = async (label, keep = false) => {
        const before = seen().length
        const conversationId = randomUUID()
        await run.client.conversation.create({
          conversationId,
          agent,
          environment: worker.name,
        })
        if (keep)
          following = await run.client.subscriptions.view(conversationId, {
            view: () => {},
            ended: () => {
              following = null
            },
          })
        const text = prompt(run, STRESS.reply, step, ` chatgpt ${label}`)
        await run.client.conversation.submit(
          run.client.conversation.prepare(conversationId, text),
        )
        const { view } = await settled(
          run,
          conversationId,
          new Set(),
          text,
          "completed",
          failures,
        )
        if (view?.lease?.host !== worker.name)
          failures.push(`its lease runs on ${view?.lease?.host}, not ${worker.name}`)
        if (failures.length === 0) failures.push(...(await atRest(run, [conversationId])))
        return { conversationId, launched: seen().slice(before) }
      }
      const handed = (launched, account, label) => {
        if (launched.length === 0) {
          failures.push(`${label}: Codex was never launched on the worker`)
          return null
        }
        for (const each of launched) {
          if (each.key)
            failures.push(`${label}: Codex was launched with an API key variable`)
          if (openai.subjectOf(each.token) !== account.subject)
            failures.push(
              `${label}: Codex's token file held ${each.token ? "a token of another account" : "nothing"}`,
            )
        }
        return launched.at(-1)
      }
      // The first account: Codex starts with its token in a file. Followed,
      // it stays open: a minute in, the gateway renews the token and the
      // machine replaces it in the same file.
      const { launched } = await talk("first", true)
      const started = handed(launched, first, "first account")
      if (!started || failures.length) return
      const renewed = await waitFor(() => {
        try {
          const now = readFileSync(started.file, "utf8")
          return now !== started.token && openai.subjectOf(now) === first.subject
        } catch {
          return false
        }
      }, 150_000)
      if (!renewed)
        failures.push("the worker's token file was not renewed while its lease ran")
      following?.close()
      following = null
      if (failures.length) return
      // The second account chosen: the next Codex runs on it.
      await run.client.logins.chatgptUse(await idOf(second))
      const other = await talk("second")
      handed(other.launched, second, "second account")
      if (failures.length) return
      // The key again: nothing of ChatGPT's is handed.
      await run.client.logins.chatgptUse(null)
      const none = await talk("none")
      if (none.launched.some((each) => each.token))
        failures.push("with no account in use Codex was still handed a ChatGPT token")
      if (secretsIn(worker.log(), openai.secrets()))
        failures.push("the worker's log holds a ChatGPT token")
    } finally {
      following?.close()
      if (worker) {
        if (run.evidence)
          writeFileSync(join(run.evidence, `worker-${worker.label}.log`), worker.log())
        await worker.close()
      }
      owner?.close()
      rmSync(launches, { force: true })
      try {
        await run.stack.vault.unlock()
        if (worker?.name) await run.client.logins.setDevice(worker.name, false)
      } catch (error) {
        failures.push(`the worker's switch could not be turned off: ${error.message}`)
      }
      await run.exclusive(async () => {
        await openLinkedDevices(run.page)
        if (await switchLinking(run, false, failures)) await closeSettings(run.page)
      })
    }
  },

  async voice(run, step, failures) {
    const realtime = run.stack.realtime
    if (!realtime) {
      failures.push("the stack was started without the fake realtime provider")
      return
    }
    const namespace = join(run.stack.gateway.directory, "ci", "instances", INSTANCE)
    const shot = (name) =>
      run.evidence
        ? run.page
            .screenshot({ path: join(run.evidence, `voice-${name}.png`) })
            .catch(() => {})
        : undefined
    const row = run.page.locator('[data-setting="voice"]')
    const pane = dock(run.page)
    const talk = pane.getByRole("button", { name: "Talk", exact: true })
    await run.exclusive(async () => {
      // With no key saved, the refused call offers to add one where it was
      // refused, instead of sending the person to Settings.
      await talk.click({ timeout: 10_000 })
      const notice = run.page.locator('.voice-dock[data-phase="failed"]')
      const add = notice.locator('[data-voice-key="add"]')
      // A locked Nessa (after a restart) offers Unlock in place, and then
      // starts the call it was asked for.
      const unlock = notice.locator('[data-voice-fix="unlock"]')
      await Promise.race([
        add.waitFor({ timeout: 15_000 }),
        unlock.waitFor({ timeout: 15_000 }),
      ]).catch(() => {})
      if (await unlock.isVisible()) {
        await shot("locked")
        await unlock.click()
        await run.page
          .locator('[data-vault-dialog="unlock"]')
          .waitFor({ timeout: 10_000 })
        await unlockVault(run.page)
      }
      if (
        !(await add.waitFor({ timeout: 15_000 }).then(
          () => true,
          () => false,
        ))
      ) {
        failures.push("a call refused for want of a key does not offer to add one")
        return
      }
      if (/Settings ›/.test((await notice.textContent()) ?? ""))
        failures.push("the missing-key notice still sends the person to Settings")
      await shot("key-missing")
      await add.click()
      const asked = run.page.locator('[data-voice-key="dialog"]')
      await asked.locator('[data-voice-key="input"]').fill(realtime.key)
      await asked.locator('[data-voice-key="save"]').click()
      const unlocking = run.page.locator('[data-vault-dialog="unlock"]')
      await waitFor(
        async () => (await unlocking.isVisible()) || !(await asked.isVisible()),
        15_000,
      )
      await unlockVault(run.page)
      if (
        !(await asked.waitFor({ state: "hidden", timeout: 15_000 }).then(
          () => true,
          () => false,
        ))
      )
        failures.push("the key dialog stayed open after Save")
      if (
        !(await notice.waitFor({ state: "hidden", timeout: 5_000 }).then(
          () => true,
          () => false,
        ))
      )
        failures.push("the missing-key notice stayed after the key was saved")
      // Settings says the key is saved, in one word, with no field to type in.
      await openAccounts(run.page)
      const saved = await run.page
        .locator('[data-setting="voice"][data-voice-key="saved"]')
        .waitFor({ timeout: 15_000 })
        .then(() => true)
        .catch(() => false)
      if (!saved) failures.push("the voice row does not say the key is saved")
      else await shot("key-saved")
      if ((await row.locator("input").count()) !== 0)
        failures.push("the voice row shows a key field instead of one button")
      await closeSettings(run.page)
    })
    if (failures.length) return
    if (!(await run.client.voice.status()).keySaved)
      failures.push("the gateway does not report the voice key saved")
    const rim = run.page.locator(".workspace-dock .voice-rim")
    const phase = () =>
      run.page
        .locator(".voice-dock")
        .getAttribute("data-phase", { timeout: 1_000 })
        .catch(() => null)
    const before = realtime.calls()
    await talk.click({ timeout: 10_000 })
    if (
      !(await waitFor(
        async () => realtime.open() && (await phase()) === "listening",
        15_000,
      ))
    ) {
      failures.push(`Talk did not open a call (phase ${await phase()})`)
      return
    }
    if (realtime.calls() !== before + 1)
      failures.push(`Talk opened ${realtime.calls() - before} calls, not one`)
    // The call is a glow on the composer's edge: its canvas lies over the
    // composer's card and lights the page just outside it. No orb is left.
    if ((await rim.count()) !== 1) failures.push("the call shows no glow on the composer")
    else {
      const boxes = await rim.evaluate((canvas) => {
        const card = canvas
          .closest(".workspace-dock")
          ?.querySelector("form.desktop-composer")
          ?.getBoundingClientRect()
        const over = canvas.getBoundingClientRect()
        const edges = (box) =>
          box ? [box.left, box.top, box.right, box.bottom].map(Math.round) : null
        return { rim: canvas.dataset.rim, over: edges(over), card: edges(card) }
      })
      // Chromium can always run the shader: a still ring there is a glow
      // that failed to start. Elsewhere the canvas may not be drawn at all.
      if (boxes.rim === "plain" && run.engine === "chromium")
        failures.push("the call's glow fell back to the still ring")
      if (
        boxes.rim !== "plain" &&
        !(
          boxes.card &&
          boxes.over[0] < boxes.card[0] &&
          boxes.over[1] <= boxes.card[1] &&
          boxes.over[2] > boxes.card[2] &&
          boxes.over[3] >= boxes.card[3]
        )
      )
        failures.push(
          `the call's glow ${JSON.stringify(boxes.over)} does not reach around the composer ${JSON.stringify(boxes.card)}`,
        )
      // Lit is read off the screen: the strip of page just left of the card
      // looks different with the glow than with it taken away, and the same
      // twice without it, so nothing else there is moving.
      const lit = await waitFor(async () => {
        if ((await rim.getAttribute("data-rim")) === "plain") return true
        const card = await pane.locator("form.desktop-composer").boundingBox()
        if (!card) return false
        const clip = {
          x: Math.round(card.x) - 6,
          y: Math.round(card.y + card.height / 2) - 10,
          width: 5,
          height: 20,
        }
        const hide = await run.page.addStyleTag({
          content: ".voice-rim { visibility: hidden !important; }",
        })
        const without = await run.page.screenshot({ clip })
        const again = await run.page.screenshot({ clip })
        await hide.evaluate((style) => style.remove())
        const withGlow = await run.page.screenshot({ clip })
        return without.equals(again) && !withGlow.equals(without)
      }, 5_000)
      if (!lit) failures.push("the call's glow lights nothing beside the composer")
    }
    if ((await run.page.locator(".voice-orb").count()) > 0)
      failures.push("the call still shows the orb")
    for (const name of ["Mute voice", "Mute microphone", "End call"])
      if (!(await pane.getByRole("button", { name, exact: true }).isVisible()))
        failures.push(`the composer shows no ${name} button during the call`)
    if ((await talk.count()) > 0) failures.push("Talk is still offered during the call")
    await expectShown(run.page, "Listening", failures)
    await shot("listening")
    // The gateway asked for the call as the coordinator, with both tools.
    const session = realtime.minted.at(-1)?.session
    const tools = (session?.tools ?? []).map((tool) => tool.name).sort()
    if (JSON.stringify(tools) !== JSON.stringify(["send_to_agent", "stop_agent"]))
      failures.push(`the call was minted with tools ${JSON.stringify(tools)}`)
    if (!/never do the work/i.test(session?.instructions ?? ""))
      failures.push("the call was not told it only coordinates")
    if (
      (run.sent.get(run.main.id) ?? 0) > 0 &&
      !(await waitFor(
        () => realtime.told().some((text) => text.startsWith("Chat so far:\n")),
        5_000,
      ))
    )
      failures.push("the voice was not handed the chat so far")

    // Said with a filler and a word twice: sent as the person said it, tidied.
    const text = prompt(run, `Please ${STRESS.reply}`, step, " voice")
    const seen = await seenTurns(run, run.main.id)
    realtime.say(
      `Um, please ${STRESS.reply} reply r${step.index} voice s${run.knobs.seed}`,
    )
    run.sent.set(run.main.id, (run.sent.get(run.main.id) ?? 0) + 1)
    await settled(run, run.main.id, seen, text, "completed", failures)
    await expectShown(run.page, `${SAYS.echo}${text}`, failures)
    const handed = await waitFor(
      () => realtime.told().some((told) => told === `Agent reply:\n${SAYS.echo}${text}`),
      15_000,
    )
    if (!handed) failures.push("the agent's reply was not handed to the voice")
    if (!realtime.outputs().includes("sent"))
      failures.push(`send_to_agent answered ${JSON.stringify(realtime.outputs())}`)
    // The voice's spoken line is the bubble's while it speaks; then it listens.
    if (!(await waitFor(async () => (await phase()) === "listening", 10_000)))
      failures.push(
        `the call is ${await phase()} after the voice answered, not listening`,
      )
    await shot("answered")

    // Words said or heard stay on one line however long they run: the
    // newest are kept, the older run off the line's start, and short words
    // sit centred. Measured on a copy of the line with words put in it, in
    // the dock's own layout, since the call speaks for a moment only.
    const lines = await run.page.evaluate(() => {
      const line = document.querySelector(".voice-dock .voice-bubble")
      if (!line) return null
      const measure = (text) => {
        const copy = line.cloneNode(true)
        copy.setAttribute("data-running", "")
        copy.querySelector(".voice-bubble-words").textContent = text
        line.after(copy)
        const box = copy.getBoundingClientRect()
        const words = copy.querySelector(".voice-bubble-words").getBoundingClientRect()
        const one = parseFloat(getComputedStyle(copy).lineHeight)
        copy.remove()
        return {
          lines: box.height / one,
          left: words.left - box.left,
          right: box.right - words.right,
        }
      }
      return {
        long: measure(
          "I can pass along anything you ask, whether it is code analysis, file checks or project digging, straight to the agent, and I will tell you what it says once it is done. ".repeat(
            3,
          ),
        ),
        short: measure("Sure, one moment."),
      }
    })
    if (!lines) failures.push("the call shows no line above the composer")
    else {
      if (lines.long.lines > 1.2 || lines.short.lines > 1.2)
        failures.push(
          `words on the call's line take ${lines.long.lines.toFixed(1)} lines when long and ${lines.short.lines.toFixed(1)} when short, not one`,
        )
      if (Math.abs(lines.long.right) > 1 || lines.long.left > -50)
        failures.push(
          `long words on the call's line do not keep their end in view (start ${Math.round(lines.long.left)}px, end ${Math.round(lines.long.right)}px inside the line)`,
        )
      if (lines.short.left < 0 || Math.abs(lines.short.left - lines.short.right) > 2)
        failures.push(
          `short words on the call's line are not centred (${Math.round(lines.short.left)}px before, ${Math.round(lines.short.right)}px after)`,
        )
    }

    // Muting the microphone throws away what it half heard.
    const clears = () =>
      realtime.received.filter((event) => event.type === "input_audio_buffer.clear")
        .length
    const cleared = clears()
    await pane.getByRole("button", { name: "Mute microphone", exact: true }).click()
    if (!(await waitFor(() => clears() === cleared + 1, 5_000)))
      failures.push("muting the microphone did not clear what it heard")
    await expectShown(run.page, "Microphone off", failures)
    await pane.getByRole("button", { name: "Unmute microphone", exact: true }).click()
    await expectShown(run.page, "Listening", failures)

    // Away from the window, the call's switches float on the pill: the same
    // call, shown in `call.html`, which the host opens as a small window over
    // other apps. Here it is another tab, and the fake host carries the
    // picture and the presses between the two as the real one does. A
    // browser's page cannot lose the front on its own, so the fake host is
    // told the window has, as the real host hears it from the system.
    const pill = await run.page.context().newPage()
    await pill.setViewportSize({ width: 300, height: 48 })
    await pill.addInitScript(gatewayHost, { endpoint: null, credential: null })
    await pill.goto(`${new URL(run.page.url()).origin}/call.html`)
    const face = pill.locator(".call-pill")
    // The page is up before it is judged empty: a pill that has not drawn
    // yet is not a pill that stayed away.
    await pill
      .locator(".call-pill-page")
      .waitFor({ timeout: 10_000 })
      .catch(() => failures.push("the pill's page did not load"))
    await sleep(300)
    if ((await face.count()) > 0)
      failures.push("the pill shows while the window with the call is in front")
    const front = (inFront) =>
      run.page.evaluate((inFront) => window.__fakeHostDesktopInFront(inFront), inFront)
    await front(false)
    const pillSays = (words) =>
      waitFor(
        async () =>
          (await face.count()) === 1 &&
          (await face.locator(".call-pill-state").innerText()).trim() === words,
        5_000,
      )
    if (!(await pillSays("Listening")))
      failures.push("away from the window, the pill does not show the listening call")
    else {
      const named = (await face.locator(".call-pill-title").innerText()).trim()
      if (named === "") failures.push("the pill names no chat")
      if (run.evidence)
        await pill.screenshot({
          path: join(run.evidence, "voice-pill.png"),
          omitBackground: true,
        })
      // The motion chosen in Settings is the pill's too: the choice is kept
      // in storage both pages share, and the pill follows a change to it.
      const pillMotion = () =>
        pill.evaluate(() => document.documentElement.dataset.motion ?? null)
      const chooseMotion = (choice) =>
        run.page.evaluate((choice) => {
          if (choice === null) localStorage.removeItem("nessa.desktop.motion")
          else localStorage.setItem("nessa.desktop.motion", choice)
        }, choice)
      const motionBefore = await run.page.evaluate(() =>
        localStorage.getItem("nessa.desktop.motion"),
      )
      await chooseMotion("reduced")
      if (!(await waitFor(async () => (await pillMotion()) === "reduced", 5_000)))
        failures.push(
          `with Reduced motion chosen the pill's page is ${await pillMotion()}, not reduced`,
        )
      await chooseMotion("full")
      if (!(await waitFor(async () => (await pillMotion()) === "full", 5_000)))
        failures.push(
          `with Full motion chosen the pill's page is ${await pillMotion()}, not full`,
        )
      await chooseMotion(motionBefore)
      // The chat's name on the pill brings the call's chat to the front of
      // the window: with another chat focused beside it, and no review
      // waiting in it, the pane showing the call takes the focus.
      await run.page.keyboard.press(keys.newSessionBeside)
      const beside = await waitFor(
        async () =>
          (await run.page.locator(css.pane).count()) === 2 &&
          (await run.page.locator(`${css.focusedPane} .voice-rim`).count()) === 0,
        10_000,
      )
      if (!beside)
        failures.push("a new session beside did not take the focus from the call's chat")
      else {
        await face.locator(".call-pill-name").click()
        if (
          !(await waitFor(
            async () =>
              (await run.page.locator(`${css.focusedPane} .voice-rim`).count()) === 1,
            5_000,
          ))
        )
          failures.push(
            "pressing the chat's name on the pill did not focus the call's chat",
          )
        // Back to one pane: the new session is focused again and closed.
        await run.page
          .locator(css.pane)
          .filter({ has: run.page.locator(css.paneHome) })
          .locator(css.field)
          .click()
        await run.page.keyboard.press(keys.closePane)
        if (
          !(await waitFor(
            async () =>
              (await run.page.locator(css.pane).count()) === 1 &&
              (await run.page.locator(`${css.focusedPane} .voice-rim`).count()) === 1,
            10_000,
          ))
        )
          failures.push("closing the session beside did not leave the call's chat alone")
      }
      // Its microphone switch is the call's own: the composer follows it,
      // and what the microphone half heard is thrown away as before.
      const pillCleared = clears()
      await face.getByRole("button", { name: "Mute microphone", exact: true }).click()
      if (!(await waitFor(() => clears() === pillCleared + 1, 5_000)))
        failures.push("muting on the pill did not mute the call")
      if (!(await pillSays("Muted"))) failures.push("the pill does not say Muted")
      if (
        !(await pane
          .getByRole("button", { name: "Unmute microphone", exact: true })
          .isVisible())
      )
        failures.push("the composer does not show the microphone the pill muted")
      await face.getByRole("button", { name: "Unmute microphone", exact: true }).click()
      if (!(await pillSays("Listening")))
        failures.push("unmuting on the pill did not bring the call back to listening")
      await face.getByRole("button", { name: "Mute voice", exact: true }).click()
      if (
        !(await pane
          .getByRole("button", { name: "Unmute voice", exact: true })
          .waitFor({ timeout: 5_000 })
          .then(() => true)
          .catch(() => false))
      )
        failures.push("the composer does not show the voice the pill muted")
      await face.getByRole("button", { name: "Unmute voice", exact: true }).click()
      await pane
        .getByRole("button", { name: "Mute voice", exact: true })
        .waitFor({ timeout: 5_000 })
        .catch(() =>
          failures.push("unmuting the voice on the pill did not reach the call"),
        )
    }
    // Back in the window, the composer has the switches and the pill goes.
    await front(true)
    if (!(await waitFor(async () => (await face.count()) === 0, 5_000)))
      failures.push("the pill stays while the window with the call is in front")
    // The person opens another chat while they talk. The call goes on: its
    // chat is in no pane now, so its switches float on the pill though the
    // window is still in front, and the call still hears its chat. What is
    // said goes to the agent, and the agent's reply is handed to the voice.
    await run.page.keyboard.press(keys.newSession)
    const left = await waitFor(
      async () =>
        (await run.page.locator(`${css.focusedPane} ${css.paneHome}`).count()) === 1 &&
        (await run.page.locator(".voice-dock").count()) === 0,
      10_000,
    )
    if (!left) failures.push("a new session did not take the place of the call's chat")
    else {
      await sleep(500)
      if (!realtime.open()) failures.push("opening another chat ended the call")
      if (!(await pillSays("Listening")))
        failures.push(
          "with the call's chat in no pane, the pill does not show over the front window",
        )
      const away = prompt(run, `Again ${STRESS.reply}`, step, " voice away")
      const awaySeen = await seenTurns(run, run.main.id)
      const awayTold = realtime.told().length
      const awayOutputs = realtime.outputs().length
      realtime.say(away)
      run.sent.set(run.main.id, (run.sent.get(run.main.id) ?? 0) + 1)
      await settled(run, run.main.id, awaySeen, away, "completed", failures)
      if (
        !(await waitFor(
          () =>
            realtime
              .told()
              .slice(awayTold)
              .some((told) => told === `Agent reply:\n${SAYS.echo}${away}`),
          15_000,
        ))
      )
        failures.push(
          "with its chat in no pane, the agent's reply was not handed to the voice",
        )
      if (!realtime.outputs().slice(awayOutputs).includes("sent"))
        failures.push(
          `with its chat in no pane, send_to_agent answered ${JSON.stringify(realtime.outputs().slice(awayOutputs))}`,
        )
      if (!(await pillSays("Listening")))
        failures.push("the pill is not listening again after the reply off screen")
      // Asked to stop off screen, the voice stops the agent's running turn.
      const awayWait = prompt(run, `Okay ${STRESS.wait}`, step, " voice away")
      const awayWaiting = await seenTurns(run, run.main.id)
      realtime.say(awayWait)
      run.sent.set(run.main.id, (run.sent.get(run.main.id) ?? 0) + 1)
      if (!(await pillSays("Working")))
        failures.push("the pill does not show the call working off screen")
      // Stopped once the agent is at work on it, as the person would hear it.
      await waitFor(async () => {
        const { view } = await readWithReviews(run.client, run.main.id).then(
          (view) => ({ view }),
          () => ({ view: null }),
        )
        const turn = view?.messages.find((each) => each.userText === awayWait)
        return turn !== undefined && !ended(turn)
      }, 15_000)
      await sleep(1_000)
      const awayStop = realtime.outputs().length
      realtime.ask("stop_agent")
      await settled(run, run.main.id, awayWaiting, awayWait, "cancelled", failures)
      if (
        !(await waitFor(
          () => realtime.outputs().slice(awayStop).includes("stopped"),
          10_000,
        ))
      )
        failures.push(
          `with its chat in no pane, stop_agent answered ${JSON.stringify(realtime.outputs().slice(awayStop))}`,
        )
      if (!(await waitFor(() => pillSays("Listening"), 30_000)))
        failures.push("the pill is not listening again after the stop off screen")
      if (run.evidence)
        await run.page
          .screenshot({ path: join(run.evidence, "voice-other-chat.png") })
          .catch(() => {})
      // The chat's name on the pill brings the chat back into the pane; the
      // composer has the switches again and the pill goes.
      await face.locator(".call-pill-name").click()
      if (
        !(await waitFor(
          async () =>
            (await run.page.locator(`${css.focusedPane} .voice-rim`).count()) === 1 &&
            (await face.count()) === 0,
          10_000,
        ))
      )
        failures.push(
          "pressing the chat's name on the pill did not bring the call's chat back and put the pill away",
        )
      if (!realtime.open()) failures.push("coming back to the call's chat ended the call")
      if ((await phase()) !== "listening")
        failures.push(`back in its chat the call is ${await phase()}, not listening`)
    }
    // One call for the window. Talk in another chat would end this call, so
    // it asks first and names the chat whose call ends: Cancel leaves the
    // call as it was, and Talk here ends it and starts one in that chat.
    // Then the same from the first chat brings the call back for what follows.
    const otherId = randomUUID()
    const otherText = prompt(run, STRESS.reply, step, " voice other chat")
    await run.client.conversation.create({
      conversationId: otherId,
      agent: run.stack.agent,
    })
    await run.client.conversation.submit(
      run.client.conversation.prepare(otherId, otherText),
    )
    await settled(run, otherId, new Set(), otherText, "completed", failures)
    run.checked.push(otherId)
    await openStressConversation(run.page, otherId, await titleOf(run, otherId))
    const otherTalk = run.page
      .locator(css.focusedPane)
      .getByRole("button", { name: "Talk", exact: true })
    const moveAsked = run.page.locator("[data-voice-move]")
    const moveHere = async (what) => {
      const calls = realtime.calls()
      await moveAsked.locator('[data-voice-move-action="confirm"]').click()
      if (
        !(await waitFor(
          async () =>
            realtime.calls() === calls + 1 &&
            realtime.open() &&
            (await run.page.locator(`${css.focusedPane} .voice-rim`).count()) === 1 &&
            (await phase()) === "listening",
          15_000,
        ))
      )
        failures.push(`Talk here did not move the call to ${what}`)
    }
    if (
      !(await otherTalk
        .waitFor({ timeout: 10_000 })
        .then(() => true)
        .catch(() => false))
    )
      failures.push("another chat offers no Talk while a call is open")
    else {
      const calls = realtime.calls()
      await otherTalk.click()
      if (
        !(await moveAsked
          .waitFor({ timeout: 5_000 })
          .then(() => true)
          .catch(() => false))
      )
        failures.push("Talk in another chat did not ask before ending the open call")
      else {
        await shot("move-asked")
        if (!(await moveAsked.innerText()).includes(run.main.title))
          failures.push("the question does not name the chat whose call would end")
        await moveAsked.getByRole("button", { name: "Cancel", exact: true }).click()
        await moveAsked.waitFor({ state: "detached", timeout: 5_000 }).catch(() => {})
        if (!realtime.open() || realtime.calls() !== calls)
          failures.push("Cancel did not leave the open call as it was")
        if (!(await pillSays("Listening")))
          failures.push("after Cancel the pill does not show the call still listening")
        // The call drops while its chat is in no pane: the chat's own notice
        // is not on screen, so the window says it, naming the chat, and the
        // pill goes with the call.
        realtime.drop()
        const endedSaid = run.page.locator("[data-voice-ended]")
        if (
          !(await endedSaid
            .waitFor({ timeout: 10_000 })
            .then(() => true)
            .catch(() => false))
        )
          failures.push("a call that dropped with its chat in no pane said nothing")
        else {
          if (!(await endedSaid.innerText()).includes(run.main.title))
            failures.push("the notice of the dropped call does not name its chat")
          await shot("ended-off-screen")
          await endedSaid.locator('[data-voice-ended-action="ok"]').click()
          await endedSaid.waitFor({ state: "detached", timeout: 5_000 }).catch(() => {})
        }
        if (!(await waitFor(async () => (await face.count()) === 0, 5_000)))
          failures.push("the pill stays after its call dropped")
        // With no call open, Talk starts one at once and asks nothing.
        const dropped = realtime.calls()
        await otherTalk.click()
        if (
          !(await waitFor(
            async () =>
              realtime.calls() === dropped + 1 &&
              realtime.open() &&
              (await run.page.locator(`${css.focusedPane} .voice-rim`).count()) === 1 &&
              (await phase()) === "listening",
            15_000,
          ))
        )
          failures.push("Talk after the dropped call did not start a call in this chat")
        if ((await moveAsked.count()) > 0)
          failures.push("Talk asked about a call that had already dropped")
        if ((await face.count()) > 0)
          failures.push("the pill shows though the pane shows the chat the call talks to")
      }
    }
    await openStressConversation(run.page, run.main.id, run.main.title)
    if (
      await talk
        .waitFor({ timeout: 10_000 })
        .then(() => true)
        .catch(() => false)
    ) {
      // The call is in the other chat now: its Talk asks the same.
      await talk.click()
      await moveAsked.waitFor({ timeout: 5_000 }).catch(() => {})
      await moveHere("the first chat")
    } else if ((await phase()) !== "listening")
      failures.push(`back in the first chat the call is ${await phase()}, not listening`)
    await front(false)
    if (!(await pillSays("Listening")))
      failures.push("the pill does not come back when the window leaves the front again")

    // Asked to stop, the voice stops the running turn and hears it stopped.
    const wait = prompt(run, `Okay ${STRESS.wait}`, step, " voice")
    const waiting = await seenTurns(run, run.main.id)
    realtime.say(wait)
    run.sent.set(run.main.id, (run.sent.get(run.main.id) ?? 0) + 1)
    await expectShown(run.page, SAYS.waiting, failures)
    if (!(await waitFor(async () => (await phase()) === "working", 10_000)))
      failures.push(`the call is ${await phase()} while the agent works, not working`)
    await shot("working")
    // A stop the person talked over is cancelled with its response: the
    // turn keeps working.
    realtime.ask("stop_agent", "cancelled")
    await sleep(1_000)
    if ((await phase()) !== "working" || realtime.outputs().at(-1) === "stopped")
      failures.push("a cancelled stop_agent stopped the turn")
    // A follow-up said while the agent works goes as a second message,
    // queued behind the running turn: the call works on until both end.
    const follow = prompt(run, `Also ${STRESS.reply}`, step, " voice")
    const told = realtime.told().length
    // Outputs since a point, so an earlier "sent" never stands for this one.
    const since = (from) => realtime.outputs().slice(from)
    const beforeFollow = realtime.outputs().length
    realtime.say(follow)
    run.sent.set(run.main.id, (run.sent.get(run.main.id) ?? 0) + 1)
    if (!(await waitFor(() => since(beforeFollow).includes("sent"), 10_000)))
      failures.push(`the follow-up answered ${JSON.stringify(since(beforeFollow))}`)
    const beforeStop = realtime.outputs().length
    realtime.ask("stop_agent")
    await settled(run, run.main.id, waiting, wait, "cancelled", failures)
    if (!(await waitFor(() => since(beforeStop).includes("stopped"), 10_000)))
      failures.push(`stop_agent answered ${JSON.stringify(since(beforeStop))}`)
    // Once the agent stops work, what it said since the first message is
    // handed over once, and the call listens again.
    const handedOnce = await waitFor(
      async () =>
        realtime
          .told()
          .slice(told)
          .filter((text) => text.startsWith("Agent reply:")).length === 1 &&
        (await phase()) === "listening",
      30_000,
    )
    if (!handedOnce)
      failures.push(
        `after the stop the call is ${await phase()} and was told ${JSON.stringify(realtime.told().slice(told))}`,
      )

    // End on the pill ends the call, as End in the composer does, and with
    // no call the pill has nothing to show.
    await pill
      .locator(".call-pill")
      .getByRole("button", { name: "End call", exact: true })
      .click()
    if (!(await waitFor(() => !realtime.open(), 10_000)))
      failures.push("End call left the call open")
    if (
      !(await waitFor(
        async () => (await pill.locator(".call-pill").count()) === 0,
        5_000,
      ))
    )
      failures.push("the pill stays after its call ended")
    await pill.close()
    await front(true)
    if (
      !(await talk.isVisible().catch(() => false)) &&
      !(await talk
        .waitFor({ timeout: 5_000 })
        .then(() => true)
        .catch(() => false))
    )
      failures.push("Talk is not offered again after End call")
    if ((await run.page.locator(".voice-dock").count()) > 0)
      failures.push("the call's line and glow stay after End call")
    if (!realtime.received.some((event) => event.type === "response.create"))
      failures.push("the voice was never asked to speak")

    // The key never left the gateway's vault: not in its log, nor its audit.
    if (run.stack.gateway.log().includes(realtime.key))
      failures.push("the gateway logged the voice key")
    const audit = join(namespace, "conversations", "audit", "voice")
    const records = existsSync(audit) ? readdirSync(audit) : []
    const kinds = records.map((name) => readFileSync(join(audit, name), "utf8"))
    for (const kind of ["voice_key_save", "voice_call"])
      if (!kinds.some((record) => record.includes(`"${kind}"`)))
        failures.push(`the voice audit has no ${kind} record`)
    if (kinds.some((record) => record.includes(realtime.key)))
      failures.push("a voice audit record holds the key")
  },

  async upgrade(run, step, failures) {
    const listed = async () =>
      (await run.client.conversation.list({})).conversations
        .map((each) => each.conversationId)
        .sort()
    await run.exclusive(async () => {
      const before = await listed()
      const path = metadataFile(run.stack.gateway.directory)
      let older
      await run.restartConsole.during(run.opened, async () => {
        const count = failures.length
        const old = run.client
        run.restarts += 1
        let earlier
        await run.stack.gateway.restart("SIGTERM", async () => {
          older = asOlderBuild(path)
          // And every place record as the two files an older build kept.
          earlier = asEarlierPlaces(conversationsOf(run))
        })
        old.close()
        run.client = await connectClient(run.stack.gateway, INSTANCE, "panel")
        log(
          `upgrade: restarted on a version ${older} file and ${earlier.length} older place files`,
        )
        if (!earlier.includes(run.main.id))
          failures.push("the main conversation had no place record to turn back")
        const version = schemaVersion(path)
        if (version !== CURRENT_METADATA_VERSION)
          failures.push(
            `the gateway left the version ${older} file at ${version}, not ${CURRENT_METADATA_VERSION}`,
          )
        const after = await listed()
        if (JSON.stringify(after) !== JSON.stringify(before))
          failures.push(
            `the upgraded gateway lists ${after.length} conversations, not the ${before.length} it did`,
          )
        // The upgraded gateway still answers the conversation, in the
        // window: that answer comes over the window's new connection, so
        // the refused connections while it was down are the restart's.
        if (failures.length === count) {
          // Spend the original Send-click budget on browser recovery and the
          // click together. The Node client above is a different connection.
          const sendBy = Date.now() + 10_000
          if (!(await run.restartConsole.readable(sendBy - Date.now())))
            failures.push(
              "the browser workspace did not recover before the upgraded Send",
            )
          else await steps.reply(run, step, failures, sendBy)
        }
        return failures.length === count
      })
    })
  },

  async launch(run, step, failures) {
    await run.exclusive(async () => {
      const page = run.page
      const removeKey = () =>
        page.evaluate((key) => sessionStorage.removeItem(key), STARTING.key)
      // The window opens while the host is still starting the gateway, as at
      // every launch: the host holds its read until the gateway is up.
      await page.evaluate((key) => sessionStorage.setItem(key, "1"), STARTING.key)
      let covered
      let askedWhileStarting
      try {
        await page.reload()
        // Whatever the window shows meanwhile, it is never the startup
        // screen: the gateway did not fail, it is starting.
        covered = await page
          .locator(css.startupScreen)
          .waitFor({ timeout: LAUNCH_STARTING_MS })
          .then(() => true)
          .catch(() => false)
        // And the window did ask while it was starting: the hold was met.
        askedWhileStarting = await page.evaluate(
          () => window.__fakeHostAsked.load_gateway_endpoint,
        )
      } finally {
        // Up, whatever happened: a later reload must not find it starting.
        // A page mid-navigation refuses the evaluate; the next one may not.
        await removeKey().catch(() => removeKey().catch(() => {}))
      }
      if (covered) {
        failures.push(
          "the window showed the startup screen while the gateway was starting",
        )
        return
      }
      if (!(askedWhileStarting > 0)) {
        failures.push("the window did not ask for the gateway while it was starting")
        return
      }
      const started = Date.now()
      const listed = await page
        .locator(selectorFor.sessionListRowFor(run.main.id))
        .waitFor({ timeout: LAUNCH_LISTED_MS })
        .then(() => true)
        .catch(() => false)
      if (!listed) {
        failures.push(
          `the window did not list the main conversation within ${LAUNCH_LISTED_MS}ms of the gateway being up`,
        )
        return
      }
      log(`launch: listed ${Date.now() - started}ms after the gateway was up`)
      if ((await page.locator(css.startupScreen).count()) > 0) {
        failures.push("the startup screen covers the window after the gateway was up")
        return
      }
      // A slow launch: startup runs past the host's hold of the window's
      // read (a busy Mac took 26.7 s against the 15 s hold). The host refuses
      // the read as still starting, which is no failure: the window must not
      // cover itself with "couldn't start", and lists the conversation once
      // the gateway is up. The hold is shortened for this launch only.
      if (!(await slowLaunch(run, failures))) return
      await unlockRequestedVault(run.client, page)
      await openStressConversation(page, run.main.id, run.main.title)
    })
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async menuPanel(run, step, failures) {
    const page = await panelWindow(run)
    try {
      const strip = page.locator(css.panelTabs)
      if (
        !(await strip.waitFor({ timeout: 30_000 }).then(
          () => true,
          () => false,
        ))
      ) {
        failures.push("the panel drew no tab strip")
        return
      }
      if ((await strip.getAttribute("data-variant")) !== "dots")
        failures.push("the panel's tabs are not page dots")
      // Messages, then the main conversation from its row: a chat opened
      // from the list takes a dot, and its name leads the strip.
      await page.locator(css.panelMessages).click()
      const rowFor = (title) =>
        page.locator(css.panelListRow).filter({ hasText: title }).first()
      if (
        !(await rowFor(run.main.title)
          .waitFor({ timeout: 15_000 })
          .then(
            () => true,
            () => false,
          ))
      ) {
        failures.push(
          `the panel's Messages does not list ${JSON.stringify(run.main.title)}`,
        )
        return
      }
      const dotsBefore = await page.locator(css.panelDot).count()
      await rowFor(run.main.title).click()
      await panelNamed(page, run.main.title, failures)
      const dots = await page.locator(css.panelDot).count()
      if (dots !== dotsBefore + 1)
        failures.push(`opening a chat left ${dots} dots, not ${dotsBefore + 1}`)
      // The selected dot is the bar: wider than every other dot, once its
      // width has settled.
      const widths = () =>
        page.locator(css.panelDot).evaluateAll((all) =>
          all.map((dot) => ({
            width: dot.getBoundingClientRect().width,
            selected: dot.closest("[data-active]") !== null,
          })),
        )
      const oneBar = (all) => {
        const bar = all.filter((each) => each.selected)
        return (
          bar.length === 1 &&
          all.every((each) => each.selected || each.width + 6 <= bar[0].width)
        )
      }
      if (!(await waitFor(async () => oneBar(await widths()), 5_000)))
        failures.push(
          `the selected dot is not the one bar: ${JSON.stringify(await widths())}`,
        )
      // A message sent from the panel wears the chat accent and its rim.
      const text = prompt(run, STRESS.reply, step, " from the panel")
      const seen = await seenTurns(run, run.main.id)
      const box = page.locator(css.panelComposer)
      await box.click()
      await page.keyboard.type(text)
      await page.keyboard.press("Enter")
      run.sent.set(run.main.id, (run.sent.get(run.main.id) ?? 0) + 1)
      await settled(run, run.main.id, seen, text, "completed", failures)
      const bubble = page.locator(css.sentBubble).filter({ hasText: text }).first()
      if (
        !(await bubble.waitFor({ timeout: 15_000 }).then(
          () => true,
          () => false,
        ))
      ) {
        failures.push("the panel does not show the message it sent")
      } else {
        // Its colours are nessa_ui's to choose: the bubble must wear the chat
        // accent and its rim, whatever values the theme gives them.
        const paint = await bubble.evaluate((node) => {
          const probe = document.createElement("span")
          probe.style.background = "var(--nessa-chat-accent)"
          probe.style.boxShadow = "var(--nessa-chat-accent-rim)"
          node.parentElement.append(probe)
          const want = getComputedStyle(probe)
          const style = getComputedStyle(node)
          const paint = {
            background: style.backgroundColor,
            accent: want.backgroundColor,
            shadow: style.boxShadow,
            rim: want.boxShadow,
          }
          probe.remove()
          return paint
        })
        if (
          paint.accent === "rgba(0, 0, 0, 0)" ||
          paint.rim === "none" ||
          paint.background !== paint.accent ||
          !paint.shadow.includes(paint.rim)
        )
          failures.push(
            `a sent bubble does not wear the chat accent and its rim: ${JSON.stringify(paint)}`,
          )
      }
      // The panel has no workspace, so its reply is looked for on the page.
      const echo = `${SAYS.echo}${text}`
      if (
        !(await page
          .getByText(echo)
          .first()
          .waitFor({ timeout: 30_000 })
          .then(
            () => true,
            () => false,
          ))
      )
        failures.push(`the panel does not show ${JSON.stringify(echo)}`)
      // A chat archived from Messages leaves a row card with its avatar,
      // and the card's Undo brings it back, in the list and the gateway.
      const conversationId = randomUUID()
      const first = prompt(run, STRESS.reply, step, " to archive")
      await run.client.conversation.create({ conversationId, agent: run.stack.agent })
      await run.client.conversation.submit(
        run.client.conversation.prepare(conversationId, first),
      )
      run.checked.push(conversationId)
      await settled(run, conversationId, new Set(), first, "completed", failures)
      const title = await titleOf(run, conversationId)
      await page.locator(css.panelMessages).click()
      const row = rowFor(title)
      if (
        !(await row.waitFor({ timeout: 15_000 }).then(
          () => true,
          () => false,
        ))
      ) {
        failures.push(`the panel's Messages does not list ${JSON.stringify(title)}`)
        return
      }
      await row.focus()
      // From the keyboard: a pointer click races the row sliding open, and a
      // list that redraws under it. Shift+F10 opens the actions and focuses
      // the first; Tab walks them.
      await page.keyboard.press("Shift+F10")
      await waitFor(
        () =>
          page.evaluate(
            () =>
              document.activeElement?.getAttribute("data-slot") ===
              "conversation-history-row-action",
          ),
        5_000,
      )
      const onArchive = () =>
        page.evaluate(
          (name) => document.activeElement?.getAttribute("aria-label") === name,
          `Archive ${title}`,
        )
      for (let tabs = 0; tabs < 4 && !(await onArchive()); tabs++)
        await page.keyboard.press("Tab")
      if (!(await onArchive())) {
        failures.push(
          `Shift+F10 on ${JSON.stringify(title)} does not reach its Archive action`,
        )
        return
      }
      await page.keyboard.press("Enter")
      const undoCard = page.locator(css.undoCard)
      if (
        !(await undoCard.waitFor({ timeout: 15_000 }).then(
          () => true,
          () => false,
        ))
      ) {
        failures.push("archiving from the panel left no undo card")
        return
      }
      if (!(await undoCard.innerText()).includes(title))
        failures.push(`the undo card does not name ${JSON.stringify(title)}`)
      if ((await undoCard.locator(css.randomAvatar).count()) !== 1)
        failures.push("the undo card does not lead with the conversation's avatar")
      const archived = async (want) =>
        waitFor(async () => {
          const { conversations } = await run.client.conversation.list({ archived: true })
          return (
            conversations.some((each) => each.conversationId === conversationId) === want
          )
        }, 15_000)
      if (!(await archived(true)))
        failures.push("the gateway did not archive the conversation")
      await undoCard.getByRole("button", { name: /^Undo/ }).click()
      if (!(await archived(false)))
        failures.push("Undo did not unarchive the conversation")
      if (
        !(await row.waitFor({ timeout: 15_000 }).then(
          () => true,
          () => false,
        ))
      )
        failures.push("Undo did not put the row back in the panel's Messages")
      if (!(await waitFor(async () => (await undoCard.count()) === 0, 5_000)))
        failures.push("the undo card stayed after Undo")
    } finally {
      await page.close()
    }
  },

  async rail(run, step, failures) {
    await run.exclusive(async () => {
      await setSideRail(run.page, true, failures)
      if (failures.length) return
      const toggle = run.page.locator(css.sideRailToggle)
      if ((await toggle.getAttribute("aria-disabled")) === "true") {
        failures.push("the window had no room for the side rail")
        return
      }
      // Open, closed, open again. Each open sends its own phrase: settled
      // counts copies of one phrase, and the first send is already there.
      let opened = 0
      let place
      for (const open of [true, false, true]) {
        if ((await toggle.getAttribute("aria-expanded")) !== String(open))
          failures.push(...(await footerThroughToggle(run.page)))
        if (!(await aroundTheLights(run.page, open ? "open" : "closed", failures))) return
        // The toggle stays under the pointer: opening or closing the rail
        // moves what is beside it, never the toggle itself.
        const box = await toggle.boundingBox()
        const at = box && [box.x, box.y, box.width, box.height].map(Math.round).join()
        place ??= at
        if (at !== place)
          failures.push(
            `the rail's toggle moved to [${at}] with the rail ${open ? "open" : "closed"}, from [${place}]`,
          )
        // The workspace still takes a message with the rail beside it.
        if (open && failures.length === 0) {
          opened += 1
          await steps.reply(
            run,
            opened === 1 ? step : { ...step, extra: " again" },
            failures,
          )
        }
      }
      await studioWithTheCorner(run.page, failures)
      await titleWithThePanesAlone(run.page, failures)
      // Turned off with the rail open: the rail goes, and back from Settings
      // the titlebar's controls start where the lights end again.
      await setSideRail(run.page, false, failures)
      await aroundTheLights(run.page, "off", failures)
    })
  },

  async panel(run, step, failures) {
    await run.exclusive(async () => {
      const page = run.page
      const panel = page.locator("#workspace-panel")
      const grid = () =>
        page.evaluate(
          () =>
            document.querySelector(".split-panes-grid")?.getBoundingClientRect().right ??
            0,
        )
      const before = await grid()
      await page.keyboard.press("Meta+Alt+KeyB")
      const opened = await waitFor(
        async () => (await panel.getAttribute("aria-hidden")) === null,
        3000,
      )
      if (!opened) {
        failures.push("⌥⌘B did not open the right panel")
        return
      }
      await page.waitForTimeout(500)
      // The panes give the panel its room; the conversation still takes a message.
      const beside = await grid()
      if (!(beside < before - 100))
        failures.push(`the panes did not make room for the panel: ${before} → ${beside}`)
      await steps.reply(run, step, failures)
      await page.getByRole("button", { name: "Expand Panel" }).click()
      if (
        !(await waitFor(
          async () => (await panel.getAttribute("data-maximized")) !== null,
          2000,
        ))
      )
        failures.push("Expand Panel did not maximize it")
      await expandedForItsChat(run, failures)
      await page.getByRole("button", { name: "Expand Panel" }).click()
      if (
        !(await waitFor(
          async () => (await panel.getAttribute("data-maximized")) !== null,
          2000,
        ))
      )
        failures.push("Expand Panel did not maximize it again")
      await page.keyboard.press("Escape")
      if (
        !(await waitFor(
          async () => (await panel.getAttribute("data-maximized")) === null,
          2000,
        ))
      )
        failures.push("Escape did not restore the maximized panel")
      await page.keyboard.press("Meta+Alt+KeyB")
      if (
        !(await waitFor(
          async () => (await panel.getAttribute("aria-hidden")) === "true",
          3000,
        ))
      )
        failures.push("⌥⌘B did not close the right panel")
      await page.waitForTimeout(500)
      const after = await grid()
      if (Math.abs(after - before) > 1)
        failures.push(
          `the panes did not take the panel's room back: ${before} → ${after}`,
        )
    })
  },

  async onboarding(run, step, failures) {
    await run.exclusive(async () => {
      const asked = await replayOnboarding(run.page, run.evidence)
      if (asked !== 1) {
        failures.push(`Replay asked the host ${asked} times, not once`)
        return
      }
      // The host opens the setup window for that ask; this page is that
      // window, over the same gateway, as the real host builds it. Both
      // agents are ready, so a click on either is a choice. The vault is
      // locked first, as after a restart: onboarding still asks for no
      // passphrase (`setupWindow` refuses a vault dialog).
      // Through the native vault session that owns it, as the app would.
      const lock = await run.stack.vault.command({ operation: "lock" })
      if (!lock.ok) throw new Error(`the vault would not lock: ${JSON.stringify(lock)}`)
      let page
      try {
        page = await setupWindow(run, (page) =>
          agentsProbe(page, () => ({ claude: "ready", codex: "ready" })),
        )
      } finally {
        // An unlock that fails is its own failure, never in place of what
        // the window did.
        await run.stack.vault
          .unlock()
          .catch((error) =>
            failures.push(`the vault did not unlock again: ${error.message}`),
          )
      }
      try {
        const heading = (name) => page.getByRole("heading", { name, exact: true })
        const button = (name) => page.getByRole("button", { name, exact: true })
        await heading(names.setup.welcome).waitFor({ timeout: 15_000 })
        await openedAsAGlow(page, failures)
        await welcomedAtOnce(page, failures)
        await setupIsGraphite(page, failures, "welcome", true)
        if (run.evidence)
          await page.screenshot({ path: join(run.evidence, "onboarding-welcome.png") })
        // Welcome has nothing before it: no Back, and Close in its corner.
        await button(names.setup.close).waitFor()
        let corners = await setupCorners(page)
        if (corners.back) failures.push("welcome shows a Back button")
        if (!inCorner(corners.close, "right"))
          failures.push(
            `Close is not round in the top-right corner: ${JSON.stringify(corners.close)}`,
          )
        // Forward and back twice: each time the picker has Back in the
        // top-left corner, and it returns to welcome.
        for (let round = 0; round < 2; round += 1) {
          await button(names.setup.getStarted).click()
          await heading(names.setup.chooseAgent).waitFor({ timeout: 10_000 })
          await button(names.setup.back).waitFor()
          corners = await setupCorners(page)
          if (!inCorner(corners.back, "left"))
            failures.push(
              `Back is not round in the top-left corner: ${JSON.stringify(corners.back)}`,
            )
          if (!inCorner(corners.close, "right"))
            failures.push(
              `on the picker Close left its corner: ${JSON.stringify(corners.close)}`,
            )
          await button(names.setup.back).click()
          await heading(names.setup.welcome).waitFor({ timeout: 10_000 })
          if (await button(names.setup.back).count())
            failures.push("back on welcome, the Back button is still shown")
        }
        // The picker lists the agents setup offers, each with its figure.
        await button(names.setup.getStarted).click()
        await heading(names.setup.chooseAgent).waitFor({ timeout: 10_000 })
        await pickerOffers(page, failures)
        await setupIsGraphite(page, failures, "picker", false)
        if (run.evidence)
          await page.screenshot({ path: join(run.evidence, "onboarding-picker.png") })
        // Close leaves setup from the picker: one handoff, as left.
        await button(names.setup.close).click()
        const handed = await waitFor(
          async () =>
            (await page.evaluate(() => window.__fakeHostSetupFinishes.length)) > 0,
          10_000,
        )
        const finishes = await page.evaluate(() => window.__fakeHostSetupFinishes)
        if (!handed) failures.push("Close did not hand setup back to the host")
        else if (finishes.length !== 1 || finishes[0].completed !== false)
          failures.push(
            `Close handed over ${JSON.stringify(finishes)}, not one left setup`,
          )
        if (await page.locator(".nessa-setup-window").count())
          failures.push("setup is still on screen after Close")
      } finally {
        await page.close()
      }
      await finishedIntoTheMenuBar(run, failures)
    })
    // The window is still the window it was: a message sent there is answered.
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async pool(run, step, failures) {
    // Workers dial the gateway's native listener, which is off in this run
    // until a step turns it on: a person does it in Settings, here as in
    // the `linking` step, and turns it off again once the workers are gone.
    const linked = await run.exclusive(async () => {
      await openLinkedDevices(run.page)
      const on = await switchLinking(run, true, failures)
      if (on) await closeSettings(run.page)
      return on
    })
    if (!linked) return
    const workers = []
    let owner
    try {
      // Pairing is the owner's: the panel's credential cannot approve.
      owner = await connectClient(run.stack.gateway, INSTANCE, "owner")
      const launch = scriptedLaunch(run.stack.agent, {
        scenario: run.stack.scenario,
        nativeContext: run.stack.nativeContext,
        model: run.stack.profile.model,
      })
      for (const label of ["a", "b"])
        workers.push(
          await startWorker({
            owner,
            address: run.nativeAddress,
            agent: run.stack.agent,
            launch,
            label: `${label}${step.index}`,
            connected: async (name) =>
              (await offers(run, (environments) => environments.includes(name))).includes(
                name,
              ),
          }),
        )
      const offered = await offers(run, (environments) =>
        workers.every((each) => environments.includes(each.name)),
      )
      if (computers(offered)[0] !== POOL) {
        failures.push(
          `the gateway offers ${JSON.stringify(offered)}, not ${POOL} before both workers`,
        )
        return
      }
      // Devices › Thread order: with nothing ordered the lowest key would
      // win a tie, so the person leaves that one out and the new
      // conversation must go to the other.
      const [leftOut] = [...workers].sort((x, y) => (x.name < y.name ? -1 : 1))
      const ordered = await run.exclusive(() =>
        setPoolInclusion(run, leftOut.name, false, failures),
      )
      if (!ordered) return
      const conversationId = randomUUID()
      await run.client.conversation.create({
        conversationId,
        agent: run.stack.agent,
        environment: POOL,
      })
      const first = prompt(run, STRESS.reply, step, " pool")
      await run.client.conversation.submit(
        run.client.conversation.prepare(conversationId, first),
      )
      const { view: before } = await settled(
        run,
        conversationId,
        new Set(),
        first,
        "completed",
        failures,
      )
      if (failures.length) return
      const bound = bindingOf(run, conversationId)
      const from = workers.find((each) => each.name === bound?.worker)
      const to = workers.find((each) => each !== from)
      if (!from || !bound.context) {
        failures.push(
          `the pool conversation is bound as ${JSON.stringify(bound)}, not to a worker with its context`,
        )
        return
      }
      if (before.lease?.environment !== "peer" || before.lease?.host !== from.name)
        failures.push(`its lease runs on ${before.lease?.host}, not ${from.name}`)
      if (from === leftOut)
        failures.push(`the pool chose ${from.name}, which the person left out`)
      // An older build kept where it runs in two files: a placement and a
      // binding. Turned back into them, the gateway reads it from those
      // until it moves.
      const conversations = conversationsOf(run)
      asEarlierPlaces(conversations, conversationId)
      if (existsSync(join(conversations, "places", `${conversationId}.json`)))
        failures.push("the place record was not turned back into an older build's files")
      // Let back in, it is where the conversation moves once its worker drains.
      const back = await run.exclusive(() =>
        setPoolInclusion(run, leftOut.name, true, failures),
      )
      if (!back) return
      // A person opens it and writes in it: it stays on the worker it is bound to.
      await openStressConversation(
        run.page,
        conversationId,
        await titleOf(run, conversationId),
      )
      await expectShown(run.page, await placeOf(from), failures)
      const stays = prompt(run, STRESS.reply, step, " pool stays")
      const kept = await send(
        run,
        conversationId,
        stays,
        failures,
        new Set(before.messages.map((each) => each.executionId)),
      )
      const { view: still } = await settled(
        run,
        conversationId,
        kept,
        stays,
        "completed",
        failures,
      )
      await expectShown(run.page, `${SAYS.echo}${stays}`, failures)
      if (JSON.stringify(bindingOf(run, conversationId)) !== JSON.stringify(bound))
        failures.push(
          `a message moved the conversation off its serving worker: ${JSON.stringify(bindingOf(run, conversationId))}`,
        )
      // Nothing changed, so nothing was written: it is still read from the
      // older build's files.
      if (existsSync(join(conversations, "places", `${conversationId}.json`)))
        failures.push(
          "a message that did not move the conversation wrote its place record",
        )
      if (failures.length) return
      // Its worker is stopped the way a container runtime stops it: it
      // drains, ends its lease, and exits on its own.
      const drainedAt = Date.now()
      if (!(await from.stop("SIGTERM", 30_000))) {
        failures.push(`worker ${from.label} did not drain and exit within 30 s`)
        return
      }
      if (from.process.exitCode !== 0)
        failures.push(
          `worker ${from.label} exited ${from.process.exitCode ?? from.process.signalCode} after draining`,
        )
      log(`pool: worker ${from.label} drained in ${Date.now() - drainedAt} ms`)
      const left = await offers(
        run,
        (environments) =>
          !environments.includes(from.name) && environments.includes(to.name),
      )
      if (!left || computers(left)[0] !== POOL)
        failures.push(`after the drain the gateway offers ${JSON.stringify(left)}`)
      // The gateway's view of it, once the drain's end of its lease is
      // there. A view asked while the gateway closes the agent that lease
      // ran is refused, as a read across a restart is; the next one is not.
      const lease = await waitFor(async () => {
        try {
          const { lease } = await followedOnce(run, conversationId)
          return lease?.state === "ended" ? lease : null
        } catch (error) {
          log(`view ${conversationId}: ${explain(error)}`)
          return null
        }
      }, 30_000)
      if (
        lease?.state !== "ended" ||
        lease.cause !== "stopped" ||
        lease.host !== from.name
      )
        failures.push(`the drained lease is ${JSON.stringify(lease)}, not ended stopped`)
      // The next message moves it: another worker, a new provider context.
      const moves = prompt(run, STRESS.reply, step, " pool moves")
      const moved = await send(
        run,
        conversationId,
        moves,
        failures,
        new Set(still?.messages.map((each) => each.executionId)),
      )
      const { view: after } = await settled(
        run,
        conversationId,
        moved,
        moves,
        "completed",
        failures,
      )
      const rebound = bindingOf(run, conversationId)
      if (rebound?.worker !== to.name)
        failures.push(`after the drain it is bound to ${rebound?.worker}, not ${to.name}`)
      // The move is its record's first change: written as one record, and
      // as the older build's binding too, so a build put back follows it.
      if (!existsSync(join(conversations, "places", `${conversationId}.json`)))
        failures.push("the move wrote no place record")
      const earlier = JSON.parse(
        readFileSync(join(conversations, "bindings", `${conversationId}.json`), "utf8"),
      )
      if (earlier.worker !== to.name)
        failures.push(`the older build's binding names ${earlier.worker}, not ${to.name}`)
      if (!rebound?.context || rebound.context === bound.context)
        failures.push(
          `after the move its provider context is ${rebound?.context}, not a new one`,
        )
      if (failures.length) return
      await expectShown(run.page, `${SAYS.echo}${moves}`, failures)
      await expectShown(run.page, await placeOf(to), failures)
      if (after?.lease?.state !== "live" || after.lease.host !== to.name)
        failures.push(
          `after the move its lease is ${after?.lease?.state} on ${after?.lease?.host}`,
        )
      // With no worker serving, the gateway refuses to read it
      // (`environment_not_configured`), so it is checked at rest here, while
      // one serves, and not in `run.checked` for the run's own rest check.
      failures.push(...(await atRest(run, [conversationId])))
      // A channel whose threads use the pool: its agent runs on the worker
      // and is told the channel and its notes, never a path of this gateway.
      await pooledChannelChat(run, step, failures)
      const stopped = Date.now()
      if (!(await to.stop("SIGTERM", 30_000)))
        failures.push(`worker ${to.label} did not drain and exit within 30 s`)
      else if (to.process.exitCode !== 0)
        failures.push(
          `worker ${to.label} exited ${to.process.exitCode ?? to.process.signalCode} after draining`,
        )
      log(`pool: worker ${to.label} drained in ${Date.now() - stopped} ms`)
      await openStressConversation(run.page, run.main.id, run.main.title)
    } finally {
      // Nothing the step started outlives it, whatever went wrong.
      for (const each of workers) {
        if (run.evidence)
          writeFileSync(join(run.evidence, `worker-${each.label}.log`), each.log())
        await each.close()
      }
      owner?.close()
      await run.exclusive(async () => {
        await openLinkedDevices(run.page)
        if (await switchLinking(run, false, failures)) await closeSettings(run.page)
      })
    }
  },

  async place(run, step, failures) {
    // A channel's new threads run on one named computer, chosen in its
    // settings from the computers connected now. Workers dial the native
    // listener, off until a step turns it on, as in `pool`.
    const linked = await run.exclusive(async () => {
      await openLinkedDevices(run.page)
      const on = await switchLinking(run, true, failures)
      if (on) await closeSettings(run.page)
      return on
    })
    if (!linked) return
    let worker
    let owner
    let channel
    try {
      owner = await connectClient(run.stack.gateway, INSTANCE, "owner")
      worker = await startWorker({
        owner,
        address: run.nativeAddress,
        agent: run.stack.agent,
        launch: scriptedLaunch(run.stack.agent, {
          scenario: run.stack.scenario,
          nativeContext: run.stack.nativeContext,
          model: run.stack.profile.model,
        }),
        label: `p${step.index}`,
        connected: async (name) =>
          (await offers(run, (environments) => environments.includes(name))).includes(
            name,
          ),
      })
      channel = await run.client.channels.create({
        requestId: `stress-place-r${step.index}-s${run.knobs.seed}`,
        name: `stress-place-r${step.index}-s${run.knobs.seed}`,
        agent: run.stack.agent,
        model: run.stack.profile.model,
      })
      // A computer the gateway cannot reach now is refused, and nothing
      // changes.
      const absent = `peer:${"0".repeat(64)}`
      const refused = await run.client.channels
        .setPlacement({
          requestId: `stress-place-absent-${channel.channelId}`,
          channelId: channel.channelId,
          coordinatorPlacement: "local",
          threadPlacement: absent,
        })
        .then(
          () => "accepted",
          (error) => error.code,
        )
      if (refused !== "channel_place_unavailable")
        failures.push(`a channel naming a computer not connected answered ${refused}`)
      // The person chooses it in the channel's settings: Computer, then
      // the worker from the computers listed.
      await run.exclusive(async () => {
        // The sidebar lists the channel once the window reads the index
        // again; a reload reads it now.
        try {
          await run.page.reload()
          // Linking restarted the gateway, which sealed its vault: the
          // reloaded window asks for it, as a person would be asked.
          await unlockRequestedVault(run.client, run.page)
          const named = run.page
            .locator(`${css.sidebar} nav`)
            .getByText(channel.name, { exact: true })
            .first()
          await named.hover({ timeout: 15_000 })
          await run.page
            .locator(
              `${css.channelSettingsOpen}[data-channel-settings-open="${channel.channelId}"]`,
            )
            .click()
          await chooseComputer()
        } catch (error) {
          if (run.evidence)
            await run.page.screenshot({
              path: join(run.evidence, `place-${run.engine}-failed.png`),
            })
          failures.push(
            `choosing the computer in the channel settings: ${explain(error)}`,
          )
        } finally {
          // Closed whatever happened, menu first: nothing after this step
          // can reach the window past an open dialog.
          const dialog = run.page.locator(css.channelSettingsDialog)
          for (let tries = 0; tries < 3 && (await dialog.count()) > 0; tries += 1) {
            await run.page.keyboard.press(keys.escape)
            await dialog.waitFor({ state: "detached", timeout: 2_000 }).catch(() => {})
          }
        }
      })
      async function chooseComputer() {
        const control = run.page
          .locator(css.channelSettingsDialog)
          .locator(`[data-channel-placement="${channel.channelId}"]`)
        const row = control.locator('[data-placement-row="threads"]')
        // Each connected computer is listed in the row's menu (#1340).
        await pickChoice(run.page, row, worker.name)
        const shown = await waitFor(
          async () =>
            (await control.getAttribute("data-thread-placement")) === worker.name,
          10_000,
        )
        if (!shown)
          failures.push(`the channel settings do not show new threads on ${worker.name}`)
        if (run.evidence)
          await run.page.screenshot({
            path: join(run.evidence, `place-${run.engine}-${channel.channelId}.png`),
          })
      }
      const { channels } = await run.client.channels.list()
      const listed = channels.find((each) => each.channelId === channel.channelId)
      if (
        listed?.threadPlacement !== worker.name ||
        listed.coordinatorPlacement !== "local"
      )
        failures.push(
          `the channel runs ${listed?.coordinatorPlacement} / ${listed?.threadPlacement}, not local / ${worker.name}`,
        )
      if (failures.length) return
      // A new chat started in it from the window, as a person would (the
      // channel chosen, ⌘N, typed and sent), runs on that computer, and
      // the window says so above its transcript.
      const text = prompt(run, STRESS.reply, step, " place")
      const conversationId = await run.exclusive(async () => {
        await run.page
          .locator(`${css.sidebar} nav`)
          .getByText(channel.name, { exact: true })
          .first()
          .click({ timeout: 10_000 })
        await run.page.keyboard.press(keys.newSession)
        const home = run.page.locator(`${css.focusedPane} ${css.paneHome}`)
        await home.locator(css.field).waitFor({ timeout: 15_000 })
        await home.locator(css.field).fill(text)
        await home.getByRole(...composerControls.send).click({ timeout: 10_000 })
        return waitFor(() => selectedStressConversation(run.page, text), 30_000)
      })
      if (!conversationId) {
        failures.push(`the chat started in ${channel.name} was never listed`)
        return
      }
      const { view } = await settled(
        run,
        conversationId,
        new Set(),
        text,
        "completed",
        failures,
      )
      const { conversations } = await run.client.conversation.list({})
      const summary = conversations.find((each) => each.conversationId === conversationId)
      if (summary?.channelId !== channel.channelId)
        failures.push(
          `the chat started in ${channel.name} is in ${summary?.channelId ?? "no channel"}`,
        )
      if (view?.lease?.environment !== "peer" || view.lease.host !== worker.name)
        failures.push(
          `a thread in a channel on ${worker.name} runs on ${view?.lease?.environment} ${view?.lease?.host}`,
        )
      const said = run.page.locator(`${css.focusedPane} [data-workspace-place="peer"]`)
      if (!(await waitFor(async () => (await said.count()) > 0, 10_000)))
        failures.push(`the window does not say the chat runs on ${worker.name}`)
      else if (run.evidence)
        await run.page.screenshot({
          path: join(run.evidence, `place-${run.engine}-chat.png`),
        })
      // Checked at rest while its computer serves: with none, the gateway
      // refuses to read it, as in `pool`.
      failures.push(...(await atRest(run, [conversationId])))
    } finally {
      if (channel)
        await run.client.channels
          .setPlacement({
            requestId: `stress-place-back-${channel.channelId}`,
            channelId: channel.channelId,
            coordinatorPlacement: "local",
            threadPlacement: "local",
          })
          .catch((error) =>
            failures.push(`the channel did not go back here: ${explain(error)}`),
          )
      if (worker) {
        if (run.evidence)
          writeFileSync(join(run.evidence, `worker-${worker.label}.log`), worker.log())
        await worker.close()
      }
      owner?.close()
      await run.exclusive(async () => {
        await openLinkedDevices(run.page)
        if (await switchLinking(run, false, failures)) await closeSettings(run.page)
      })
      // Switching linking off restarted the gateway, which sealed its vault:
      // unlocked again, so the steps after this one find it as it was.
      await run.stack.vault.unlock()
      // Back to the main conversation, in the default channel: the chat
      // above left the session list showing the step's channel.
      await run.page
        .locator(`${css.sidebar} nav`)
        .getByText("Conversations", { exact: true })
        .first()
        .click({ timeout: 10_000 })
        .catch(() => {})
      await openStressConversation(run.page, run.main.id, run.main.title)
    }
  },

  async devices(run, step, failures) {
    // A worker dials the native listener, off until a step turns it on:
    // here Add a computer does, as a person with linking off would.
    const linked = await run.exclusive(() => reachFromDevices(run, failures))
    if (!linked) return
    // With evidence asked for, each page is photographed as it is checked.
    const photograph = (label) =>
      run.evidence
        ? run.page
            .screenshot({
              path: join(run.evidence, `devices-${run.engine}-${label}.png`),
            })
            .catch(() => {})
        : Promise.resolve()
    let worker
    let owner
    try {
      owner = await connectClient(run.stack.gateway, INSTANCE, "owner")
      worker = await startWorker({
        owner,
        address: run.nativeAddress,
        agent: run.stack.agent,
        launch: scriptedLaunch(run.stack.agent, {
          scenario: run.stack.scenario,
          nativeContext: run.stack.nativeContext,
        }),
        label: `devices${step.index}`,
        connected: async (name) =>
          (await offers(run, (environments) => environments.includes(name))).includes(
            name,
          ),
      })
      const id = worker.name.slice("peer:".length)
      // The gateway's list names the worker with the machine it reported
      // over its lease link, as the window reads it.
      const listed = await waitFor(async () => {
        const { workers } = await run.client.devices.list()
        const found = workers.find((each) => each.environment === worker.name)
        return found?.state === "online" && found.machine ? found : null
      }, 30_000)
      if (!listed) {
        failures.push(`devices.list never named ${worker.name} online with its machine`)
        return
      }
      if (
        !(listed.machine.cpus > 0 && listed.machine.memoryTotalBytes > 0) ||
        !listed.machine.disks?.length ||
        listed.machine.diskTotalBytes !==
          Math.min(
            Number.MAX_SAFE_INTEGER,
            listed.machine.disks.reduce((sum, disk) => sum + disk.totalBytes, 0),
          )
      )
        failures.push(`the worker reported ${JSON.stringify(listed.machine)}`)
      // Each page follows the last; together they name the worker once.
      const pages = []
      for (let after; pages.length < 1_024;) {
        const page = await run.client.devices.list(after)
        pages.push(page)
        if (!page.next) break
        after = page.next
      }
      const named = pages.flatMap((page) => page.workers).map((each) => each.environment)
      if (named.filter((name) => name === worker.name).length !== 1)
        failures.push(`devices.list pages name the worker ${JSON.stringify(named)}`)
      await run.exclusive(async () => {
        const page = run.page
        await setSideRail(page, true, failures)
        if (failures.length) return
        await page.locator('[data-rail-item="devices"]').click()
        const tile = page.locator(`[data-devices-worker="${id}"]`)
        if (
          !(await waitFor(
            async () => (await page.$("[data-devices-hub]")) !== null,
            10_000,
          ))
        )
          failures.push("the Devices view drew no hub card")
        // It keeps "nessa Studio", the way to Settings, in the bottom-left
        // corner as Agents does, and its page fades out before that corner.
        if (
          !(await page.locator(".side-rail-studio .desktop-identity-button").isVisible())
        )
          failures.push(`the Devices view draws no "nessa Studio" in its corner`)
        const faded = await page.evaluate(
          () =>
            getComputedStyle(document.querySelector(".devices-page") ?? document.body)
              .maskImage,
        )
        if (!faded || faded === "none")
          failures.push(`the Devices page does not fade out under "nessa Studio"`)
        // Its titlebar's free room moves the window, as over a chat.
        const drags = await page.evaluate(() => {
          const space = document.querySelector(".workspace-titlebar-space")
          if (!space) return false
          const box = space.getBoundingClientRect()
          const hit = document.elementFromPoint(
            box.left + box.width / 2,
            box.top + box.height / 2,
          )
          return hit?.closest("[data-tauri-drag-region]") !== null && hit !== null
        })
        if (!drags) failures.push("the Devices titlebar does not move the window")
        for (const stat of ["CPU", "Memory", "Disk"])
          if (
            (await page
              .locator(`[data-devices-hub] [data-devices-stat="${stat}"]`)
              .count()) !== 1
          )
            failures.push(`the hub card has no ${stat}`)
        const online = await waitFor(async () => {
          const text = (await tile.count()) ? await tile.innerText() : ""
          return text.includes(listed.machine.hostname) &&
            text.includes("Online") &&
            text.includes("CPU")
            ? text
            : null
        }, 15_000)
        if (!online) {
          failures.push(
            `the worker's tile reads ${JSON.stringify(await tile.allInnerTexts())}`,
          )
          return
        }
        await photograph("list")
        // The filled action's words stand out from its fill.
        const ink = await page
          .locator('[data-devices-action="add"]')
          .evaluate((button) => {
            const style = getComputedStyle(button)
            return { fill: style.backgroundColor, ink: style.color }
          })
        if (ink.fill === ink.ink)
          failures.push(`Add device is drawn ${ink.ink} on ${ink.fill}`)
        // Its own page: its load, never the agents it runs.
        await tile.click()
        const detail = page.locator('[data-devices-page="worker"]')
        await detail.waitFor({ timeout: 5_000 })
        for (const stat of ["Agents", "CPU", "Memory", "Disk"])
          if ((await detail.locator(`[data-devices-stat="${stat}"]`).count()) !== 1)
            failures.push(`the worker's page has no ${stat}`)
        const volumes = await detail
          .locator("[data-devices-disks] .devices-stat-label")
          .allInnerTexts()
        const reported = listed.machine.disks.map((disk) => disk.mount)
        if (
          JSON.stringify(volumes) !== JSON.stringify(reported.length > 1 ? reported : [])
        )
          failures.push(`the worker's page names disks ${JSON.stringify(volumes)}`)
        if ((await detail.innerText()).includes("Running here"))
          failures.push("the worker's page lists what runs there")
        await photograph("worker")
        await detail.locator('[data-devices-action="back"]').click()
        // One list: the computer and the cloud side by side, no sections.
        const list = page.locator("[data-devices-list]")
        await list.waitFor({ timeout: 5_000 })
        if (
          (await list.locator(`[data-devices-worker="${id}"]`).count()) !== 1 ||
          (await list.locator('[data-devices-cloud-id="boat"]').count()) !== 1
        )
          failures.push("the worker and the cloud are not in one list")
        if (
          (await list
            .locator('[data-devices-cloud-id="boat"] [data-slot=device-figure]')
            .getAttribute("data-kind")) !== "boat"
        )
          failures.push("Boat's tile is not drawn as a boat")
        // One title with nothing under it, one Add device, and Boat's state
        // said once, in one word.
        // The kit's PageHeader, in the same place as Settings' title.
        const header = await page.evaluate(() => {
          const head = document.querySelector(
            '[data-devices-page="list"] > [data-slot="page-header"]',
          )
          return head
            ? {
                next: head.nextElementSibling?.tagName ?? null,
                size: getComputedStyle(head.querySelector("h1")).fontSize,
              }
            : null
        })
        if (!header) failures.push("Devices does not start with the page header")
        else if (header.next === "P")
          failures.push("the Devices title still has a line under it")
        if ((await page.locator('[data-devices-action^="add"]').count()) !== 1)
          failures.push("Devices offers Add device more than once")
        const boatSays = await list.locator('[data-devices-cloud-id="boat"]').innerText()
        if (/Offline/.test(boatSays) && /Not connected/.test(boatSays))
          failures.push(`Boat's tile says its state twice: ${boatSays}`)
        // The `ssh` step's suggestions are a section of their own, under it.
        const headings = await page
          .locator(
            '[data-devices-page="list"] h2:not(#devices-hub-name):not(#devices-suggestions-title)',
          )
          .allInnerTexts()
        if (headings.length) failures.push(`Devices splits its list: ${headings}`)
        // Boat is managed on its own page, not sent to Settings.
        if (
          (await page.locator('[data-devices-page="list"]').innerText()).includes(
            "Settings ›",
          )
        )
          failures.push("Devices still sends Boat to Settings")
        await list.locator('[data-devices-cloud-id="boat"]').click()
        const cloud = page.locator('[data-devices-page="cloud"]')
        await cloud.waitFor({ timeout: 5_000 })
        // It says where Boat stands, and offers the one thing to do about it:
        // unlock while the vault is locked, else a live token field.
        const settled = await waitFor(async () => {
          const word = await cloud
            .locator("[data-devices-cloud-state]")
            .getAttribute("data-devices-cloud-state")
          if (word === "Unavailable") return word
          if (word === "Locked")
            return (await cloud
              .locator('[data-boat-action="unlock"]')
              .isEnabled({ timeout: 500 })
              .catch(() => false))
              ? word
              : null
          if (word === "Connected" || word === "Not connected")
            return (await cloud
              .locator('input[type="password"]')
              .isEnabled({ timeout: 500 })
              .catch(() => false))
              ? word
              : null
          return null
        }, 10_000)
        if (!settled)
          failures.push(
            `Boat's page reads ${await cloud.locator("[data-devices-cloud-state]").innerText()} without its action`,
          )
        // Boat is drawn as a boat, on its tile and its page, and floats only
        // while connected.
        const boat = await cloud
          .locator("[data-slot=device-figure]")
          .evaluate((figure) => ({
            kind: figure.getAttribute("data-kind"),
            afloat: figure.hasAttribute("data-afloat"),
          }))
        if (boat.kind !== "boat" || boat.afloat !== (settled === "Connected"))
          failures.push(`Boat's page draws ${JSON.stringify(boat)} while ${settled}`)
        await photograph("cloud")
        await cloud.locator('[data-devices-action="back"]').click()
        // Add device offers each kind; a computer gets a code and the two commands that use it.
        await page.locator('[data-devices-action="add"]').click()
        const kinds = await page
          .locator("[data-devices-add]")
          .evaluateAll((each) =>
            each.map((button) => button.getAttribute("data-devices-add")),
          )
        if (JSON.stringify(kinds) !== JSON.stringify(["computer", "phone", "cloud"]))
          failures.push(`Add device offers ${JSON.stringify(kinds)}`)
        // Add device › Cloud offers every cloud, each opening its own page.
        await page.locator('[data-devices-add="cloud"]').click()
        const offered = await page
          .locator("[data-devices-add-cloud]")
          .evaluateAll((each) =>
            each.map((button) => button.getAttribute("data-devices-add-cloud")),
          )
        if (JSON.stringify(offered) !== JSON.stringify(["boat", "agent37"]))
          failures.push(`Add device › Cloud offers ${JSON.stringify(offered)}`)
        await page.locator('[data-devices-add-cloud="agent37"]').click()
        if (
          !(await page
            .locator('[data-devices-cloud-page="agent37"] [data-setting="agent37"]')
            .waitFor({ timeout: 5_000 })
            .then(
              () => true,
              () => false,
            ))
        )
          failures.push("Add device › Cloud › Agent37 does not open Agent37's page")
        // A cloud's own page never offers another cloud: the default cloud
        // is chosen once, in Settings › Accounts.
        if (
          (await page
            .locator('[data-devices-cloud-page="agent37"] [data-cloud-choice]')
            .count()) > 0
        )
          failures.push("Agent37's page offers a choice of cloud")
        await photograph("cloud-agent37")
        await page.locator('[data-devices-action="back"]').click()
        await page.locator('[data-devices-action="add"]').click()
        // A phone pairs right here: Settings' rows, drawn as cards on Devices,
        // and the code's modal on top of the page, giving focus back on Escape.
        await page.locator('[data-devices-add="phone"]').click()
        const phone = page.locator('[data-devices-page="add-phone"]')
        await phone.waitFor({ timeout: 5_000 })
        const pair = phone.locator(selectorFor.linkedAction("pair"))
        if (
          !(await waitFor(
            async () => (await pair.count()) === 1 && (await pair.isEnabled()),
            10_000,
          ))
        )
          failures.push("Add a phone offers no live Pair a device")
        else {
          const card = await phone
            .locator('[data-slot="settings-group-card"]')
            .first()
            .evaluate((node) => {
              const style = getComputedStyle(node)
              return {
                fill: style.backgroundColor,
                radius: parseFloat(style.borderTopLeftRadius),
              }
            })
            .catch(() => null)
          // Its rows are drawn as Settings draws them: flat on the page, as
          // rows with hairlines, not a card (the kit's flat SettingsGroup).
          if (!card || card.fill !== "rgba(0, 0, 0, 0)" || card.radius !== 0)
            failures.push(
              `Add a phone draws its rows in a card, not flat as Settings does: ${JSON.stringify(card)}`,
            )
          await photograph("phone")
          await pair.click()
          const dialog = page.locator(css.pairingDialog)
          const code = await waitFor(
            async () => (await dialog.locator(css.pairingCode).count()) > 0,
            10_000,
          )
          if (!code) failures.push("Add a phone's Pair a device opened no code")
          else {
            const onTop = await dialog.evaluate((node) => {
              const box = node.getBoundingClientRect()
              const hit = document.elementFromPoint(
                box.left + box.width / 2,
                box.top + box.height / 2,
              )
              return hit !== null && node.contains(hit)
            })
            if (!onTop) failures.push("the pairing modal is drawn under the Devices page")
            await photograph("phone-code")
            await page.keyboard.press("Escape")
            await dialog
              .waitFor({ state: "detached", timeout: 10_000 })
              .catch(() =>
                failures.push("Escape did not close the pairing modal on Devices"),
              )
            const focusedIn = () =>
              page.evaluate(() => ({
                page:
                  document.activeElement
                    ?.closest("[data-devices-page]")
                    ?.getAttribute("data-devices-page") ?? null,
                tag: document.activeElement?.tagName ?? null,
                text: document.activeElement?.textContent?.slice(0, 40) ?? null,
              }))
            const back = await waitFor(
              async () => ((await focusedIn()).page === "add-phone" ? true : null),
              2_000,
            )
            const focused = back ? "add-phone" : JSON.stringify(await focusedIn())
            if (focused !== "add-phone")
              failures.push(
                `closing the pairing modal left focus on ${focused ?? "the body"}`,
              )
            // Leave no code open: cancel it from its row.
            await phone.locator(selectorFor.linkedAction("cancel")).click()
            await pair
              .waitFor({ timeout: 10_000 })
              .catch(() =>
                failures.push("Pair a device was not offered again after cancelling"),
              )
          }
        }
        await phone.locator('[data-devices-action="back"]').click()
        await page.locator('[data-devices-action="add"]').click()
        await page.locator('[data-devices-add="computer"]').click()
        const add = page.locator('[data-devices-page="add"]')
        const shown = await waitFor(
          async () => (await add.locator("[data-devices-command]").count()) === 2,
          10_000,
        )
        const lines = await add.locator("[data-devices-command] code").allInnerTexts()
        if (
          !shown ||
          !lines[0]?.startsWith("nessa env enroll ") ||
          !lines[1]?.startsWith("nessa env dial ")
        )
          failures.push(`Add a computer shows ${JSON.stringify(lines)}`)
        await photograph("add")
        await add.locator('[data-devices-action="back"]').click()
      })
      if (failures.length) return
      // Stopped, it stays listed, offline, with when it was last seen.
      if (!(await worker.stop("SIGTERM", 30_000)))
        failures.push(`worker ${worker.label} did not drain and exit within 30 s`)
      await run.exclusive(async () => {
        const page = run.page
        const tile = page.locator(`[data-devices-worker="${id}"]`)
        const offline = await waitFor(async () => {
          const text = (await tile.count()) ? await tile.innerText() : ""
          return text.includes("Offline") && text.includes("Last seen") ? text : null
        }, 15_000)
        if (!offline)
          failures.push(
            `the stopped worker's tile reads ${JSON.stringify(await tile.allInnerTexts())}`,
          )
        await photograph("offline")
        await page.locator('[data-rail-item="agents"]').click()
        await setSideRail(page, false, failures)
      })
    } finally {
      if (worker) {
        if (run.evidence)
          writeFileSync(join(run.evidence, `worker-${worker.label}.log`), worker.log())
        await worker.close()
      }
      owner?.close()
      await run.exclusive(async () => {
        await openLinkedDevices(run.page)
        if (await switchLinking(run, false, failures)) await closeSettings(run.page)
      })
    }
  },

  async ssh(run, step, failures) {
    const gateway = run.stack.gateway
    const config = join(gateway.directory, "ci", "instances", INSTANCE, "config.json")
    const sshConfig = join(gateway.directory, "home", ".ssh", "config")
    const pid = gateway.server.pid
    const photograph = (label) =>
      run.evidence
        ? run.page
            .screenshot({ path: join(run.evidence, `ssh-${run.engine}-${label}.png`) })
            .catch(() => {})
        : Promise.resolve()
    // The person's SSH configuration, read as Devices is opened, naming
    // machines no earlier run of this step connected.
    const hosts = nextSshHosts()
    const { reachable: SSH_REACHABLE, unreachable: SSH_UNREACHABLE } = hosts
    writeSshConfig(join(gateway.directory, "home"), hosts)
    try {
      const { suggested } = await run.client.devices.sshHosts()
      const names = suggested.map((each) => each.host)
      if (JSON.stringify(names) !== JSON.stringify([SSH_REACHABLE, SSH_UNREACHABLE]))
        failures.push(`the gateway suggests ${JSON.stringify(names)}`)
      const reachable = suggested.find((each) => each.host === SSH_REACHABLE)
      if (
        reachable?.hostName !== "10.0.0.7" ||
        reachable.user !== "builder" ||
        reachable.port !== 2222
      )
        failures.push(`${SSH_REACHABLE} is suggested as ${JSON.stringify(reachable)}`)
      if (failures.length) return
      const conversationId = randomUUID()
      await run.exclusive(async () => {
        const page = run.page
        await setSideRail(page, true, failures)
        if (failures.length) return
        await page.locator('[data-rail-item="devices"]').click()
        const pills = page.locator("[data-devices-suggested]")
        if (!(await waitFor(async () => (await pills.count()) === 2, 15_000))) {
          failures.push(
            `Devices suggests ${JSON.stringify(await pills.allInnerTexts())}, not two machines`,
          )
          return
        }
        const pill = (host) => page.locator(`[data-devices-suggested="${host}"]`)
        if (!(await pill(SSH_REACHABLE).innerText()).includes("builder@10.0.0.7"))
          failures.push(
            `${SSH_REACHABLE}'s pill reads ${await pill(SSH_REACHABLE).innerText()}`,
          )
        // Connect shows on hover; a machine nothing reaches says so and can be tried again.
        await pill(SSH_UNREACHABLE).hover()
        await pill(SSH_UNREACHABLE).locator('[data-devices-action="connect"]').click()
        const failed = await waitFor(
          async () =>
            (await pill(SSH_UNREACHABLE).getAttribute("data-state")) === "failed" &&
            (await pill(SSH_UNREACHABLE).innerText()).includes("Try again"),
          30_000,
        )
        if (!failed)
          failures.push(
            `an unreachable machine's pill reads ${await pill(SSH_UNREACHABLE).innerText()}`,
          )
        // Its words stand out from its fill, as Add device's do.
        const ink = await pill(SSH_UNREACHABLE)
          .locator('[data-devices-action="connect"]')
          .evaluate((button) => {
            const style = getComputedStyle(button)
            return { fill: style.backgroundColor, ink: style.color }
          })
        if (ink.fill === ink.ink)
          failures.push(`Try again is drawn ${ink.ink} on ${ink.fill}`)
        await photograph("suggested")
        // Connecting installs this build there, saves it and restarts the
        // gateway in place; the machine moves from the pills to the list.
        await run.restartConsole.during(run.opened, async () => {
          const before = failures.length
          const old = run.client
          run.restarts += 1
          await pill(SSH_REACHABLE).hover()
          await pill(SSH_REACHABLE).locator('[data-devices-action="connect"]').click()
          const tile = page.locator(`[data-devices-ssh="${SSH_REACHABLE}"]`)
          const listed = await waitFor(
            async () =>
              (await tile.count()) === 1 && (await pill(SSH_REACHABLE).count()) === 0,
            180_000,
          )
          if (!listed)
            failures.push(
              `after Connect, ${SSH_REACHABLE}'s pill reads ${JSON.stringify(await pill(SSH_REACHABLE).allInnerTexts())}`,
            )
          else if (!(await tile.innerText()).includes(`SSH · ${SSH_REACHABLE}`))
            failures.push(`${SSH_REACHABLE}'s tile reads ${await tile.innerText()}`)
          old.close()
          run.client = await connectClient(gateway, INSTANCE, "panel")
          const saved = JSON.parse(readFileSync(config, "utf8")).sshHosts
          if (!saved?.includes(SSH_REACHABLE))
            failures.push(`config.json holds sshHosts ${JSON.stringify(saved)}`)
          if (gateway.server.pid !== pid || gateway.server.exitCode !== null)
            failures.push("the gateway did not start again in the same process")
          return failures.length === before
        })
        if (failures.length) return
        // The restarted gateway keeps no word of the failed try: the pill
        // offers Connect again rather than holding an old answer.
        if (
          !(await waitFor(
            async () =>
              (await pill(SSH_UNREACHABLE).getAttribute("data-state")) === "idle",
            15_000,
          ))
        )
          failures.push(
            `after the restart, ${SSH_UNREACHABLE}'s pill reads ${await pill(SSH_UNREACHABLE).innerText()}`,
          )
        const offered = await offers(run, (environments) =>
          environments.includes(SSH_REACHABLE),
        )
        if (!offered.includes(SSH_REACHABLE))
          failures.push(
            `the gateway offers ${JSON.stringify(offered)}, not ${SSH_REACHABLE}`,
          )
        // The star makes it where new chats run, for every client.
        const holder = page.locator(".devices-tile-holder", {
          has: page.locator(`[data-devices-ssh="${SSH_REACHABLE}"]`),
        })
        await holder.hover()
        const star = page.locator(`[data-devices-default="${SSH_REACHABLE}"]`)
        await star.click()
        if (
          !(await waitFor(
            async () => (await star.getAttribute("data-chosen")) !== null,
            10_000,
          ))
        )
          failures.push(`${SSH_REACHABLE}'s star is not chosen once pressed`)
        await photograph("default")
        const chosen = await waitFor(async () => {
          const { defaultEnvironment } = await run.client.agents.list()
          return defaultEnvironment === SSH_REACHABLE ? defaultEnvironment : null
        }, 10_000)
        if (!chosen) failures.push(`agents.list names no default ${SSH_REACHABLE}`)
        if ((await run.client.devices.list()).defaultEnvironment !== SSH_REACHABLE)
          failures.push(`devices.list names no default ${SSH_REACHABLE}`)
        if (JSON.parse(readFileSync(config, "utf8")).defaultEnvironment !== SSH_REACHABLE)
          failures.push("config.json keeps no default device")
        // A chat that names no place runs there.
        await run.client.conversation.create({ conversationId, agent: run.stack.agent })
        run.checked.push(conversationId)
        const text = prompt(run, STRESS.reply, step, " ssh")
        await run.client.conversation.submit(
          run.client.conversation.prepare(conversationId, text),
        )
        const leased = await waitFor(async () => {
          const view = await readWithReviews(run.client, conversationId).catch(() => null)
          return view?.lease?.host === SSH_REACHABLE ? view.lease : null
        }, 120_000)
        if (!leased) {
          const view = await readWithReviews(run.client, conversationId).catch(() => null)
          failures.push(
            `a chat with no place has lease ${JSON.stringify(view?.lease)}, not on ${SSH_REACHABLE}`,
          )
        }
        // Back to the hub: its star, and a new chat stays here again.
        await page.locator('[data-devices-default="hub"]').click()
        const back = await waitFor(async () => {
          const { defaultEnvironment } = await run.client.agents.list()
          return defaultEnvironment === undefined
        }, 10_000)
        if (!back) failures.push("the hub's star did not take the default back")
        await page.locator('[data-rail-item="agents"]').click()
        await setSideRail(page, false, failures)
      })
      if (failures.length) return
      await settled(
        run,
        conversationId,
        new Set(),
        prompt(run, STRESS.reply, step, " ssh"),
        "completed",
        failures,
        120,
      )
    } finally {
      rmSync(sshConfig, { force: true })
      await run.client.devices.setDefault(null).catch(() => {})
    }
  },

  async tunnel(run, step, failures) {
    const directory = tunnelDirectory(run)
    const on = await run.exclusive(async () => {
      await openLinkedDevices(run.page)
      if (!(await switchLinking(run, true, failures))) return false
      if (!(await switchTunnel(run, true, failures))) return false
      await closeSettings(run.page)
      return true
    })
    // Its own function, so a check that ends it early still turns it off.
    const throughTunnel = async () => {
      let worker
      let owner
      try {
        const { address } = await run.client.pairing.tunnel()
        const kept = keptTunnel(directory)
        if (!address || !kept) {
          failures.push(
            `the ready tunnel is at ${address} with ${JSON.stringify(kept)} kept`,
          )
          return
        }
        // The gateway stops and starts again; cloudflared does not.
        await run.exclusive(() =>
          run.restartConsole.during(run.opened, async () => {
            const before = failures.length
            const old = run.client
            run.restarts += 1
            await run.stack.gateway.restart("SIGTERM")
            old.close()
            run.client = await connectClient(run.stack.gateway, INSTANCE, "panel")
            const again = await waitFor(async () => {
              const read = await run.client.pairing.tunnel()
              return read.status === "ready" ? read : null
            }, 60_000)
            if (again?.address !== address)
              failures.push(
                `after a restart the tunnel is at ${again?.address}, not ${address}`,
              )
            const after = keptTunnel(directory)
            if (after?.pid !== kept.pid || !alive(kept.pid))
              failures.push(
                `the restarted gateway did not take cloudflared ${kept.pid} back: ${JSON.stringify(after)}`,
              )
            if (!(await run.restartConsole.readable(10_000)))
              failures.push(
                "the window did not regain a readable session after tunnel restart",
              )
            return failures.length === before
          }),
        )
        if (failures.length) return
        // A paired machine that cannot reach the gateway directly dials it
        // through the tunnel.
        const through = tunnelConnections(directory)
        owner = await connectClient(run.stack.gateway, INSTANCE, "owner")
        worker = await startWorker({
          owner,
          address: run.nativeAddress,
          agent: run.stack.agent,
          launch: scriptedLaunch(run.stack.agent, {
            scenario: run.stack.scenario,
            nativeContext: run.stack.nativeContext,
          }),
          label: `t${step.index}`,
          tunnel: { address: UNREACHABLE, origin: address },
          connected: async (name) =>
            (await offers(run, (environments) => environments.includes(name))).includes(
              name,
            ),
        })
        if (tunnelConnections(directory) <= through)
          failures.push("the worker connected, but not through the tunnel")
        const conversationId = randomUUID()
        await run.client.conversation.create({
          conversationId,
          agent: run.stack.agent,
          environment: worker.name,
        })
        const text = prompt(run, STRESS.reply, step, " tunnel")
        await run.client.conversation.submit(
          run.client.conversation.prepare(conversationId, text),
        )
        const { view } = await settled(
          run,
          conversationId,
          new Set(),
          text,
          "completed",
          failures,
        )
        if (view?.lease?.host !== worker.name)
          failures.push(`its lease runs on ${view?.lease?.host}, not ${worker.name}`)
        // Read at rest while its worker serves: the run's own rest check
        // comes after the worker is gone.
        if (failures.length === 0) failures.push(...(await atRest(run, [conversationId])))
      } finally {
        if (worker) {
          if (run.evidence)
            writeFileSync(join(run.evidence, `worker-${worker.label}.log`), worker.log())
          await worker.close()
        }
        owner?.close()
      }
    }
    // Off again, whatever happened: the tunnel's program ends with it.
    // A throw from the check still turns the tunnel off first: left on, the
    // gateway would start a new program after the kill below, and nothing
    // here would know of it.
    let thrown = null
    try {
      if (on) await throughTunnel()
    } catch (error) {
      thrown = error
    }
    let kept = null
    try {
      kept = keptTunnel(directory)
      await run.exclusive(async () => {
        await openLinkedDevices(run.page)
        const tunnelOn =
          (await run.page
            .locator(css.tunnel)
            .getAttribute("data-tunnel-enabled")
            .catch(() => null)) === "true"
        if (tunnelOn && (await switchTunnel(run, false, failures)) && kept) {
          const gone = await waitFor(async () => !alive(kept.pid), 15_000)
          if (!gone)
            failures.push(`cloudflared ${kept.pid} outlived the tunnel turned off`)
        }
        if (
          (await run.page.locator(css.linked).getAttribute("data-linked")) === "on" &&
          (await switchLinking(run, false, failures))
        )
          await closeSettings(run.page)
      })
    } finally {
      // Nothing the step started outlives it: its own process group, so
      // the gateway's end never reaches it.
      const left = keptTunnel(directory) ?? kept
      if (left && alive(left.pid)) {
        failures.push(`cloudflared ${left.pid} was still running at the step's end`)
        try {
          process.kill(-left.pid, "SIGKILL")
        } catch {
          // Gone in the meantime.
        }
      }
    }
    if (thrown) throw thrown
    if (failures.length === 0) await steps.reply(run, step, failures)
  },

  async delegate(run, step, failures) {
    const text = prompt(run, STRESS.delegate, step)
    const seen = await send(run, run.main.id, text, failures)
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    if (!turn) return
    await expectShown(run.page, SAYS.delegated, failures)
    // The thread's turn ends, and its report wakes this conversation.
    const report = await waitFor(async () => {
      const view = await run.client.conversation.read(run.main.id)
      return view.messages.find(
        (each) =>
          !seen.has(each.executionId) &&
          each.userText.startsWith(STRESS.report) &&
          ended(each),
      )
    }, 90_000)
    if (!report)
      failures.push("the thread's report never reached the conversation that sent it")
    else if (report.status !== "completed")
      failures.push(`the report's turn ended ${report.status}`)
    else {
      await expectShown(run.page, SAYS.report, failures)
      await reportLine(run, report, failures)
      // Copy through the person's Fork command; the report keeps its author
      // in the saved handoff, copied transcript and the next model's history.
      const listed = new Set(
        (await run.client.conversation.list({})).conversations.map(
          (each) => each.conversationId,
        ),
      )
      await choose(run.page, run.main.id, "Fork")
      const forkId = await waitFor(async () => {
        const { conversations } = await run.client.conversation.list({})
        return conversations.find((each) => !listed.has(each.conversationId))
          ?.conversationId
      }, 15_000)
      if (!forkId) {
        failures.push("the agent-report conversation never forked")
        return
      }
      run.checked.push(forkId)
      const forked = await run.client.conversation.read(forkId)
      const copied = forked.handoffs
        .flatMap((each) => each.turns ?? [])
        .find((each) => each.said === report.userText)
      if (
        copied?.agent?.conversationId !== report.agent?.conversationId ||
        copied?.agent?.executionId !== report.agent?.executionId
      )
        failures.push("the fork's saved report lost its agent origin")
      const line = run.page
        .locator(css.focusedPane)
        .locator(css.threadReport)
        .filter({
          has: run.page.locator(
            `${css.chatPill}[data-chat-pill="${report.agent?.conversationId}"]`,
          ),
        })
        .first()
      if (
        !(await line
          .waitFor({ state: "visible", timeout: 15_000 })
          .then(() => true)
          .catch(() => false))
      )
        failures.push("the copied report lost its agent label in the fork")
      else if (run.evidence)
        await line.screenshot({
          path: join(run.evidence, `fork-agent-report-${run.engine}.png`),
        })
      const inFork = prompt(run, STRESS.reply, step, " after agent report fork")
      const forkSeen = await send(run, forkId, inFork, failures, new Set())
      const { turn: next } = await settled(
        run,
        forkId,
        forkSeen,
        inFork,
        "completed",
        failures,
      )
      const reply = (next?.parts ?? []).map((part) => part.text ?? "").join("")
      if (!reply.includes(`"agent":${JSON.stringify(report.agent)}`))
        failures.push(
          "the fork's first model input lost the copied report's typed agent origin",
        )
      await openStressConversation(run.page, run.main.id, run.main.title)
    }
  },

  async await(run, step, failures) {
    await awaitThread(run, step, run.main.id, failures)
  },

  async watch(run, step, failures) {
    const text = prompt(run, STRESS.watch, step)
    const seen = await send(run, run.main.id, text, failures)
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    if (turn) watched(turn, failures)
  },

  async window(run, step, failures) {
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " beside")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    run.checked.push(conversationId)
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    await settled(run, conversationId, new Set(), first, "completed", failures)
    if (failures.length) return
    // ⌘B hides the sidebar, which makes room for a pane beside; ⌘-click on a
    // row in the session list then opens it there, focused. The page is a Mac's.
    const folded = await lookHolds(run.page, "folding the sidebar")
    await run.page.keyboard.press(keys.toggleSidebar)
    await sidebarParks(run.page, failures, false)
    await folded(failures)
    await run.page
      .locator(selectorFor.sessionListRowFor(conversationId), {
        hasText: await titleOf(run, conversationId),
      })
      .click({
        modifiers: ["Meta"],
        timeout: 15_000,
      })
    if (
      !(await waitFor(
        async () => (await run.page.locator(css.pane).count()) === 2,
        10_000,
      ))
    ) {
      failures.push(
        `opening beside left ${await run.page.locator(css.pane).count()} panes, not 2`,
      )
      return
    }
    await flatPanes(run.page, failures)
    // Asked in the composer of the pane beside, the focused one.
    const beside = await windowRead(
      run,
      conversationId,
      step,
      " beside",
      failures,
      (said) =>
        said.you?.position === "right" &&
        said.you.focused === true &&
        said.windows?.length === 1 &&
        said.windows[0].sidebar === "closed" &&
        said.windows[0].sessionList === "open" &&
        said.windows[0].panes.some(
          (pane) =>
            pane.threadId === run.main.id &&
            pane.position === "left" &&
            pane.focused === false,
        ),
    )
    if (beside) {
      const window = beside.windows[0]
      if (window.arrangement !== "columns" || window.showing !== "panes")
        failures.push(
          `the window reads as ${window.arrangement} showing ${window.showing}`,
        )
      const left = window.panes.find((pane) => pane.threadId === run.main.id)
      if (
        left?.you !== false ||
        left?.available === false ||
        left?.kind !== "conversation"
      )
        failures.push(`the main conversation's pane reads ${JSON.stringify(left)}`)
    }
    if (failures.length) return
    // The focused chat's panel: two tabs, the front one, hidden and shown,
    // then the other chat. read_window is what the agent is told.
    await focusedChatPanel(run, conversationId, step, failures)
    if (failures.length) return
    // One header per chat, in the titlebar's row — beside the session list,
    // and with the panes alone at the window's edge.
    await headersLand(run.page, "beside the session list", failures)
    await run.page.keyboard.press(keys.toggleSessionList)
    await waitFor(
      async () =>
        (await run.page.locator(`${css.workspace}[data-panes-alone]`).count()) === 1,
      5_000,
    )
    await headersLand(run.page, "with the panes alone", failures)
    await run.page.keyboard.press(keys.toggleSessionList)
    await waitFor(
      async () =>
        (await run.page.locator(`${css.workspace}[data-panes-alone]`).count()) === 0,
      5_000,
    )
    if (failures.length) return
    // ⌘W closes the focused pane: the one beside.
    const closed = await lookHolds(run.page, "closing the pane beside")
    await run.page.keyboard.press(keys.closePane)
    if (
      !(await waitFor(
        async () => (await run.page.locator(css.pane).count()) === 1,
        10_000,
      ))
    ) {
      failures.push("⌘W did not close the pane beside")
      return
    }
    await closed(failures)
    // A new session beside names itself: alone, a home is its greeting;
    // beside another pane every pane says what it is (UI audit #27).
    await homeBesideNamed(run.page, failures)
    await noHistoryButtons(run.page, failures, "the window")
    if (failures.length) return
    const unfolded = await lookHolds(run.page, "opening the sidebar")
    await run.page.keyboard.press(keys.toggleSidebar)
    await sidebarParks(run.page, failures, true)
    await unfolded(failures)
    await channelShowsList(run.page, failures)
    if (failures.length) return
    // The main conversation is the pane left. Not opened from its row: the
    // row beside it quotes the window, the main conversation's title with it.
    // Asked from the main conversation's composer: it is the whole window now,
    // the session list shown again by the channel.
    await windowRead(
      run,
      run.main.id,
      step,
      " alone",
      failures,
      (said) =>
        said.you?.position === "full" &&
        said.you.focused === true &&
        said.windows?.length === 1 &&
        said.windows[0].sessionList === "open" &&
        said.windows[0].panes.length === 1,
    )
  },

  async control(run, step, failures) {
    run.controlPane = undefined
    // A second conversation, for the agent in the main one to open beside it.
    const other = randomUUID()
    const first = prompt(run, STRESS.reply, step, " controlled")
    await run.client.conversation.create({
      conversationId: other,
      agent: run.stack.agent,
    })
    run.checked.push(other)
    await run.client.conversation.submit(run.client.conversation.prepare(other, first))
    await settled(run, other, new Set(), first, "completed", failures)
    if (failures.length) return
    const panes = () => run.page.locator(css.pane).count()
    const hidden = (selector) =>
      run.page.evaluate(
        (selector) => document.querySelector(selector)?.getAttribute("aria-hidden"),
        selector,
      )
    // Each change is the main conversation's agent calling the tool: the
    // window showing it carries it out by its own rules and says what came
    // of it once it shows it, so the tool's answer is already on screen.
    const asked = (phrase, args, extra, options) =>
      windowControl(run, phrase, args, step, extra, failures, options)

    // Folding the sidebar makes room for a pane beside, as ⌘B does.
    let said = await asked(STRESS.arrangeWindow, { sidebar: "closed" }, " fold")
    if (said?.window?.windows?.[0]?.sidebar !== "closed")
      failures.push(`arrange_window said ${JSON.stringify(said)}`)
    if (!(await waitFor(async () => (await hidden(css.sidebar)) === "true", 5_000)))
      failures.push("arrange_window left the sidebar shown")
    said = await asked(STRESS.openThread, { threadId: other, where: "beside" }, " open")
    // The publication the answer waited for comes from the store; the panes
    // are painted after it.
    const shows = (count) => waitFor(async () => (await panes()) === count, 5_000)
    if (said?.changed !== true || !(await shows(2)))
      failures.push(
        `open_thread beside said ${JSON.stringify(said)} with ${await panes()} panes on screen`,
      )
    if (failures.length) return
    said = await asked(STRESS.arrangeWindow, { panel: "open" }, " panel")
    if (said?.changed !== true)
      failures.push(`opening the panel said ${JSON.stringify(said)}`)
    if (!(await waitFor(async () => (await hidden(css.panel)) !== "true", 5_000)))
      failures.push("arrange_window left the right panel hidden")
    // Asked again, nothing changes, and the agent is told so.
    said = await asked(STRESS.arrangeWindow, { sidebar: "closed" }, " again")
    if (said?.changed !== false)
      failures.push(`a repeated arrange said ${JSON.stringify(said)}`)
    said = await asked(
      STRESS.arrangeWindow,
      { sidebar: "open", panel: "closed" },
      " unfold",
    )
    if (said?.changed !== true)
      failures.push(`arrange_window back said ${JSON.stringify(said)}`)
    await sidebarParks(run.page, failures, true)
    if (!(await waitFor(async () => (await hidden(css.panel)) === "true", 5_000)))
      failures.push("arrange_window left the right panel shown")
    if (failures.length) return
    // The Agents overview over the panes, then the panes again.
    await asked(STRESS.arrangeWindow, { view: "agents" }, " overview")
    if (
      !(await waitFor(
        async () => (await run.page.locator(css.overview).count()) === 1,
        5_000,
      ))
    )
      failures.push("arrange_window did not show the Agents overview")
    // The overview covers every composer, so this one turn is sent the way
    // another of the person's devices would send it: on the client.
    await asked(STRESS.arrangeWindow, { view: "panes" }, " panes", { covered: true })
    if (!(await shows(2)))
      failures.push(`back on the panes, ${await panes()} are on screen`)
    // A tab the focused chat's panel does not hold is refused, not guessed.
    said = await asked(
      STRESS.showPanelTab,
      { plugin: "stress", id: "no-such-tab" },
      " missing",
    )
    if (said?.error !== "tab_not_found")
      failures.push(`show_panel_tab of a missing tab said ${JSON.stringify(said)}`)
    said = await asked(STRESS.focusPane, { threadId: run.main.id }, " focus")
    const focused = said?.window?.windows?.[0]?.panes?.find((pane) => pane.focused)
    if (focused?.threadId !== run.main.id)
      failures.push(`focus_pane said ${JSON.stringify(said)}`)
    if (failures.length) return
    // Closing takes the other conversation's pane away; a second close finds
    // it no longer on screen.
    said = await asked(STRESS.closePane, { threadId: other }, " close")
    if (said?.changed !== true || !(await shows(1)))
      failures.push(`close_pane said ${JSON.stringify(said)} with ${await panes()} panes`)
    said = await asked(STRESS.closePane, { threadId: other }, " gone")
    if (said?.error !== "not_on_screen")
      failures.push(`closing a pane not on screen said ${JSON.stringify(said)}`)
    // Every change is on record: asked, then what came of it.
    const namespace = join(run.stack.gateway.directory, "ci", "instances", INSTANCE)
    const audit = join(namespace, "conversations", "audit", "window-control")
    const records = existsSync(audit)
      ? readdirSync(audit).map((name) =>
          JSON.parse(readFileSync(join(audit, name), "utf8")),
        )
      : []
    const closes = records.filter(
      (record) =>
        record.action?.kind === "close_pane" && record.action.conversationId === other,
    )
    const outcomes = closes.filter((record) => record.phase === "outcome")
    if (
      closes.filter((record) => record.phase === "requested").length < 1 ||
      !outcomes.some((record) => record.outcome?.outcome === "applied") ||
      !outcomes.some((record) => record.outcome?.code === "not_on_screen") ||
      closes.some((record) => record.severity !== "destructive")
    )
      failures.push(`the window-control audit holds ${JSON.stringify(closes)}`)
  },

  async peer(run, step, failures) {
    // The owner sees the result in the window. The note and the turn are
    // sent by the paired gateway, on its own credential, after the owner
    // grants comment and drive. The composer's Send would be the owner.
    const note = `peer note ${step.index}`
    const drive = `${STRESS.reply} peer drive ${step.index}`
    const conversationId = randomUUID()
    const root = mkdtempSync(join(tmpdir(), "nessa-stress-peer-"))
    let linked = false
    try {
      linked = await run.exclusive(async () => {
        await openLinkedDevices(run.page)
        const on = await switchLinking(run, true, failures)
        if (on) await closeSettings(run.page)
        return on
      })
      if (!linked || failures.length) return
      await run.client.conversation.create({
        conversationId,
        agent: run.stack.agent,
      })
      run.checked.push(conversationId)
      const stateRoot = join(root, "state")
      const cache = join(root, "cache")
      mkdirSync(stateRoot, { mode: 0o700 })
      mkdirSync(cache, { mode: 0o700 })
      const profile = join(root, "profile.json")
      writeFileSync(
        profile,
        JSON.stringify({
          stateRoot,
          stateDirectory: "device",
          cache,
          gatewayAddress: run.nativeAddress,
        }),
        { mode: 0o600 },
      )
      const { code, status } = await run.client.pairing.create({ enrollee: "gateway" })
      const enrollment = await enrollPeer(run, profile, code, status.invitationId)
      await run.client.conversation.share(conversationId, enrollment.credentialId, [
        "read",
        "comment",
      ])
      await run.client.conversation.share(
        conversationId,
        enrollment.credentialId,
        ["read", "comment", "drive"],
        { preset: "read_only_tools" },
      )
      const commented = peerExample(["comment", profile, conversationId, note])
      if (!commented.outcome?.successful)
        failures.push(
          `the peer's note was refused: ${commented.outcome?.refusal ?? commented.stderr ?? commented.error?.message ?? commented.status}`,
        )
      const driven = peerExample(["drive", profile, conversationId, drive])
      if (!driven.outcome?.successful)
        failures.push(
          `the peer's turn was refused: ${driven.outcome?.refusal ?? driven.stderr ?? driven.error?.message ?? driven.status}`,
        )
      if (failures.length) return
      await settled(run, conversationId, new Set(), drive, "completed", failures)
      const view = await run.client.conversation.read(conversationId)
      const saved = view.notes.find((each) => each.text === note)
      if (saved?.author.kind !== "grant")
        failures.push(
          `the gateway's view does not show the peer's note as a grant: ${JSON.stringify(saved?.author ?? null)}`,
        )
      await run.exclusive(async () => {
        await openStressConversation(
          run.page,
          conversationId,
          await titleOf(run, conversationId),
        )
        await expectShown(run.page, note, failures)
        await expectShown(run.page, drive, failures)
        if ((await run.page.locator('[data-note-author="grant"]').count()) < 1)
          failures.push("the window does not show the peer's note as a grant")
        if (run.evidence)
          await run.page
            .screenshot({
              path: join(run.evidence, `peer-${step.index}.png`),
              scale: "css",
            })
            .catch(() => {})
      })
      const uses = join(
        run.stack.gateway.directory,
        "ci",
        "instances",
        INSTANCE,
        "access-grants",
        "grant-uses",
      )
      const records = existsSync(uses)
        ? readdirSync(uses).map((name) =>
            JSON.parse(readFileSync(join(uses, name), "utf8")),
          )
        : []
      const mine = records.filter(
        (record) => record.target?.conversationId === conversationId,
      )
      for (const action of ["conversation.comment", "conversation.drive"]) {
        const recorded = mine.find((record) => record.target?.action === action)
        if (!recorded?.grant?.principalId?.startsWith("gateway:"))
          failures.push(
            `the grant record for ${action} does not name the peer: ${JSON.stringify(recorded ?? null)}`,
          )
      }
      log(
        `peer ${enrollment.credentialId} commented and drove ${conversationId} at epoch ${enrollment.accessEpoch}`,
      )
    } catch (error) {
      failures.push(`a granted peer could not comment and drive: ${error.message}`)
    } finally {
      if (linked) {
        await run.exclusive(async () => {
          await openStressConversation(run.page, run.main.id, run.main.title)
          await openLinkedDevices(run.page)
          if (await switchLinking(run, false, failures)) await closeSettings(run.page)
        })
      }
      rmSync(root, { recursive: true, force: true })
    }
  },

  async webhook(run, step, failures) {
    // The panel creates this conversation and holds the default owner grants,
    // including credential.manage. Sharing must use that same conversation owner.
    const owner = await connectClient(run.stack.gateway, INSTANCE, "panel")
    const conversationId = randomUUID()
    let shared = false
    try {
      if (!run.stack.webhookCredential) {
        const { organizationId, gatewayId } = await owner.auth.session()
        const principalId = "stress-webhook"
        const issued = await owner.credentials.issue({
          principal: { id: principalId, kind: "integration" },
          membership: {
            id: "stress-webhook-membership",
            principalId,
            organizationId,
            role: "member",
            state: "active",
          },
          expiresAt: null,
          grants: [
            { action: "conversation.read", resource: { organizationId, id: gatewayId } },
          ],
        })
        if (!("secretUnavailable" in issued) || issued.secretUnavailable !== true)
          throw new Error("webhook issue must withhold its synthetic credential secret")
        run.stack.webhookCredential = issued.credential.id
      }
      const credential = run.stack.webhookCredential
      await run.client.conversation.create({ conversationId, agent: run.stack.agent })
      run.checked.push(conversationId)
      const setup = prompt(run, STRESS.reply, step, " webhook setup")
      await run.client.conversation.submit(
        run.client.conversation.prepare(conversationId, setup),
      )
      await settled(run, conversationId, new Set(), setup, "completed", failures)
      if (failures.length) return
      await owner.conversation.share(conversationId, credential, ["read"])
      shared = true
      const sameScope = (a, b) =>
        ["receiver", "origin", "stream", "incarnation", "schema", "accessEpoch"].every(
          (key) => a[key] === b[key],
        )
      const catchUp = async (fixed, deadline = Date.now() + 15_000) => {
        if (
          !(await waitFor(
            async () => {
              const native = await owner.conversation.webhookCursor(
                conversationId,
                credential,
              )
              if (native.resetRequired || !sameScope(native.scope, fixed.scope))
                throw new Error(
                  "webhook capture changed incarnation or requires explicit reset",
                )
              return BigInt(native.position) >= BigInt(fixed.head)
            },
            Math.max(0, deadline - Date.now()),
          ))
        )
          throw new Error("webhook native cursor did not reach the fixed source Head")
      }
      // The first source cycle may not yet have installed a cursor. Poll the
      // native diagnostic only; a bounded refusal is not treated as caught up.
      let lastRefusal
      if (
        !(await waitFor(async () => {
          try {
            await owner.conversation.webhookCursor(conversationId, credential)
            return true
          } catch (error) {
            if (error.code !== "invalid_request") throw error
            lastRefusal = error
            return false
          }
        }, 15_000))
      )
        throw new Error("webhook cursor was never installed", { cause: lastRefusal })
      const baseline = (
        await owner.conversation.webhookCursor(conversationId, credential, {
          includeSourceHead: true,
        })
      ).observedSourceHead
      await catchUp(baseline)
      const title = await titleOf(run, conversationId)
      await openStressConversation(run.page, conversationId, title)
      const text = prompt(run, STRESS.webhook, step)
      const seen = await send(run, conversationId, text, failures)
      await settled(run, conversationId, seen, text, "completed", failures)
      await expectShown(run.page, `${SAYS.webhook}${text}`, failures)
      if (failures.length) return
      // Capture once after this turn commits. Poll only the native cursor;
      // observing newer Heads on each iteration would chase a moving target.
      const fixed = (
        await owner.conversation.webhookCursor(conversationId, credential, {
          includeSourceHead: true,
        })
      ).observedSourceHead
      if (
        !sameScope(baseline.scope, fixed.scope) ||
        BigInt(fixed.head) <= BigInt(baseline.head)
      )
        throw new Error(
          "browser webhook witness did not advance the same source incarnation",
        )
      const deadline = Date.now() + 15_000
      await catchUp(fixed, deadline)
      const proof = webhookNoAttempt(
        run.stack.gateway,
        INSTANCE,
        credential,
        fixed,
        deadline,
      )
      log(
        `webhook captured fixed Head ${fixed.head}; ${proof.queued} queued, ${proof.attempted} attempted; loopback policy unchanged`,
      )
    } finally {
      try {
        if (shared)
          await owner.conversation.unshare(conversationId, run.stack.webhookCredential)
      } catch (error) {
        failures.push(`webhook grant cleanup failed: ${error.message}`)
      } finally {
        owner.close()
        await openStressConversation(run.page, run.main.id, run.main.title)
      }
    }
  },

  async aside(run, step, failures) {
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " aside")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    await settled(run, conversationId, new Set(), first, "completed", failures)
    const title = await titleOf(run, conversationId)
    await openStressConversation(run.page, conversationId, title)
    const second = prompt(run, STRESS.reply, step, " aside again")
    const seen = await send(run, conversationId, second, failures)
    await settled(run, conversationId, seen, second, "completed", failures)
    await expectShown(run.page, `${SAYS.echo}${second}`, failures)
    await openStressConversation(run.page, run.main.id, run.main.title)
  },

  async channel(run, step, failures) {
    const notes = `Answer as the stress channel s${run.knobs.seed}.`
    // In the run's own folder, removed with it: the rest check reads these
    // channels' conversations again at the end, and an agent cannot be built
    // in a folder that is gone.
    const attached = mkdtempSync(join(run.stack.scratch, "channel-"))
    const resource = join(attached, "brief.md")
    writeFileSync(resource, "# Brief\n")
    // Dragged onto the window: a file and a folder, as the host names them.
    writeFileSync(join(attached, "dropped.md"), "# Dropped\n")
    mkdirSync(join(attached, "dropped-kit"))
    const dropped = [
      { path: realpathSync(join(attached, "dropped.md")), kind: "file" },
      { path: realpathSync(join(attached, "dropped-kit")), kind: "folder" },
    ]
    const chosen = mkdtempSync(join(run.stack.scratch, "chosen-"))
    try {
      // Made in a folder of its own, after a folder agents may not use.
      const made = await makeChannel(run, failures, {
        name: `stress-made-r${step.index}-s${run.knobs.seed}`,
        refusedFolder: "/",
        notes,
        resource,
        dropped,
      })
      if (made) {
        if (!made.folderMade || !made.folder.includes(`/channels/${made.channelId}`))
          failures.push(
            `the channel made without a folder works in ${made.folder}, not one made for it`,
          )
        if (!existsSync(made.folder))
          failures.push(`the made folder ${made.folder} is not there`)
        await channelRuntimeSettings(run, made, failures)
        const talked = await talkInChannel(
          run,
          step,
          made,
          failures,
          notes,
          dropped.map(({ path }) => path),
        )
        await run.exclusive(() => channelRowsKeepHover(run.page, failures))
        // A thread its agent makes naming where it runs stays in the
        // channel.
        const thread =
          talked && (await awaitThread(run, step, talked, failures, ` ${made.name}`))
        if (thread) {
          const { conversations } = await run.client.conversation.list({})
          const listed = conversations.find((each) => each.conversationId === thread)
          if (listed?.channelId !== made.channelId)
            failures.push(
              `a thread made in ${made.name} naming its place is in ${listed?.channelId ?? "no channel"}`,
            )
        }
        // Forked from its row, the fork stays in the channel. (That it runs
        // where its source runs, not where the channel moved, is held by
        // the gateway's fork tests: nothing runs on Boat in this run.)
        if (talked) await forkInChannel(run, made, talked, failures)
        // Talked to, its run location changes from its settings.
        await channelPlacementSettings(run, made, failures)
      }
      // Made in a folder chosen in the host's picker.
      const there = await makeChannel(run, failures, {
        name: `stress-chosen-r${step.index}-s${run.knobs.seed}`,
        folder: chosen,
      })
      if (there) {
        if (there.folderMade || there.folder !== realpathSync(chosen))
          failures.push(
            `the channel made in ${chosen} works in ${there.folder} (made: ${there.folderMade})`,
          )
        await talkInChannel(run, step, there, failures)
      }
    } finally {
      // Back to the main conversation, in the default channel.
      await run.page
        .locator(`${css.sidebar} nav`)
        .getByText("Conversations", { exact: true })
        .first()
        .click({ timeout: 10_000 })
        .catch(() => {})
      await openStressConversation(run.page, run.main.id, run.main.title)
    }
  },

  async fresh(run, step, failures) {
    // A person starts a conversation the way the window does: a new
    // session's home, its first message typed and sent. The window creates
    // the conversation and sends that message itself, with no client library
    // in between. Meanwhile `FRESH_BURST` other conversations are created and
    // sent to at once, so the window's create and send arrive while many
    // other commands are still in flight: a gateway that refuses a command
    // for the commands around it, rather than queueing it, fails here.
    const text = prompt(run, STRESS.reply, step, ` fresh ${run.engine}`)
    // Spread over clients of their own: one socket takes only so many
    // requests at once, and what this step asks about is the gateway's whole.
    // Each conversation is also read on its own client, for the same reason.
    const clients = []
    const burst = []
    try {
      for (let n = 0; n < FRESH_CLIENTS; n += 1)
        clients.push(await connectClient(run.stack.gateway, INSTANCE, "panel"))
      for (let n = 0; n < FRESH_BURST; n += 1)
        burst.push(startedBeside(run, clients[n % FRESH_CLIENTS], step, n, failures))
      await run.page.keyboard.press(keys.newSession)
      const home = run.page.locator(`${css.focusedPane} ${css.paneHome}`)
      await home.locator(css.field).waitFor({ timeout: 15_000 })
      // Home has its own selection. Pick the named fixture through the window,
      // never inherit an unrelated default agent or substitute another model.
      const model = catalog.models.find((row) => row.modelId === run.stack.profile.model)
      await selectStressModel(run.page, home.locator(css.modelChip), model.displayName)
      await home.locator(css.field).fill(text)
      await home.getByRole(...composerControls.send).click({ timeout: 10_000 })
      const conversationId = await waitFor(
        () => selectedStressConversation(run.page, text),
        30_000,
      )
      if (!conversationId) {
        failures.push("the conversation started in the window was never listed")
      } else {
        run.checked.push(conversationId)
        const { view } = await settled(
          run,
          conversationId,
          new Set(),
          text,
          "completed",
          failures,
        )
        if (view?.runtime?.model !== run.stack.profile.model)
          failures.push("the new window conversation did not use the exact fixture model")
        await expectShown(run.page, `${SAYS.echo}${text}`, failures)
      }
      // Answered or not, a send the window took and then failed says so in
      // its composer, with the code it was refused with.
      const refused = run.page.locator(css.sendFailure)
      if ((await refused.count()) > 0)
        failures.push(
          `the first message of a new conversation failed: ${fold(await refused.first().innerText())}`,
        )
    } finally {
      // A step that threw leaves no burst running into the next one.
      await Promise.allSettled(burst)
      for (const client of clients) client.close()
    }
    await openStressConversation(run.page, run.main.id, run.main.title)
  },

  async notice(run, step, failures) {
    const conversationId = randomUUID()
    run.checked.push(conversationId)
    await run.client.conversation.create({
      conversationId,
      agent: run.stack.agent,
      approvalMode: "ask",
    })
    // Named by a first turn: the gateway records a conversation's title just
    // after the agent takes its first message, and the scripted agent can
    // ask before that lands, which a model never does.
    const naming = prompt(run, STRESS.reply, step, " notice")
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, naming),
    )
    const { view: named } = await settled(
      run,
      conversationId,
      new Set(),
      naming,
      "completed",
      failures,
    )
    if (failures.length) return
    const title = await titleOf(run, conversationId)
    // The person is on the main conversation: the review waits where the
    // window does not show it, and only the notice says so.
    const first = prompt(run, STRESS.permission, step, " notice")
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    const posted = await noticePosted(run, conversationId, failures)
    if (!posted) return
    if (posted.notice.body !== title)
      failures.push(
        `the notice names ${JSON.stringify(posted.notice.body)}, not the conversation ${JSON.stringify(title)}`,
      )
    if (posted.notice.title !== `${posted.review.toolName} needs a decision`)
      failures.push(
        `the notice says ${JSON.stringify(posted.notice.title)}, not that ${posted.review.toolName} needs a decision`,
      )
    // The person clicks the notice: the host sends its event and the window
    // opens that conversation on its card.
    const listening = await run.page.evaluate(
      (id) => window.__fakeHostOpenNotice?.(id) ?? 0,
      conversationId,
    )
    if (listening !== 1)
      failures.push(`${listening} windows listened for an opened notice, not 1`)
    const opened = await run.page
      .locator(`${selectorFor.sessionListRowFor(conversationId)}[aria-selected=true]`)
      .waitFor({ timeout: 15_000 })
      .then(() => true)
      .catch(() => false)
    if (!opened) {
      failures.push("opening the notice did not bring its conversation up")
      return
    }
    await answerCard(run, conversationId, "allow", failures, "Report the rows")
    await noticeTakenBack(run, posted.notice, "answered in the window", failures)
    const { view: before } = await settled(
      run,
      conversationId,
      new Set(named?.messages.map((each) => each.executionId) ?? []),
      first,
      "completed",
      failures,
    )
    await expectShown(run.page, SAYS.allowed, failures)
    if (failures.length) return
    // Back on the main conversation, a second review there is answered by
    // another of the person's clients: the window was never shown it, and
    // its notice still goes.
    await openStressConversation(run.page, run.main.id, run.main.title)
    const second = prompt(run, STRESS.permission, step, " notice elsewhere")
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, second),
    )
    const again = await noticePosted(run, conversationId, failures, posted.notice)
    if (!again) return
    // The window is reloaded while that review waits. The gateway sends the
    // waiting set as soon as the new socket is up, before the window listens;
    // the window must still hand it to the host, which keeps the notice up.
    await run.page.reload()
    await run.page
      .locator(selectorFor.sessionListRowFor(run.main.id))
      .waitFor({ timeout: 30_000 })
    const kept = await waitFor(async () => {
      const notices = await hostNotices(run.page)
      return (
        notices !== null &&
        notices.syncs > 0 &&
        notices.up.some((each) => sameNotice(each, again.notice))
      )
    }, 15_000)
    if (!kept) {
      failures.push(
        "after a reload the window did not hand the waiting review to the host",
      )
      return
    }
    await unlockRequestedVault(run.client, run.page)
    await openStressConversation(run.page, run.main.id, run.main.title)
    const { permissionTargets } = await import("@nessa/client")
    const view = await readWithReviews(run.client, conversationId)
    const index = view.permissions.findIndex(
      (each) => each.permissionId === again.review.permissionId,
    )
    const deny = view.permissions[index]?.options.find((each) => each.effect === "deny")
    if (!deny) {
      failures.push("the second review offers no deny")
      return
    }
    await run.client.conversation.answer(permissionTargets(view)[index], deny.id)
    await noticeTakenBack(run, again.notice, "answered elsewhere", failures)
    await settled(
      run,
      conversationId,
      new Set(before?.messages.map((each) => each.executionId) ?? []),
      second,
      "completed",
      failures,
    )
  },

  async ask(run, step, failures) {
    const conversationId = randomUUID()
    run.checked.push(conversationId)
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    // Named by a first turn, so the notice has a title to say.
    const naming = prompt(run, STRESS.reply, step, " ask")
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, naming),
    )
    const { view: named } = await settled(
      run,
      conversationId,
      new Set(),
      naming,
      "completed",
      failures,
    )
    if (failures.length) return
    const title = await titleOf(run, conversationId)
    const shown = await run.page
      .locator(`${selectorFor.sessionListRowFor(run.main.id)}[aria-selected=true]`)
      .count()
    const asking = prompt(run, STRESS.ask, step)
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, asking),
    )
    const { view: asked } = await settled(
      run,
      conversationId,
      new Set(named?.messages.map((each) => each.executionId) ?? []),
      asking,
      "completed",
      failures,
    )
    if (failures.length) return
    const rowAsk = async () => {
      const { conversations } = await run.client.conversation.list({})
      return conversations.find((each) => each.conversationId === conversationId)?.ask
    }
    const ask = await waitFor(rowAsk, 15_000)
    if (!ask) {
      failures.push("the list row does not carry the agent's ask")
      return
    }
    if (
      ask.question !== ASK.question ||
      ask.choices.map((each) => each.label).join("|") !==
        ASK.choices.map((each) => each.label).join("|") ||
      ask.recommended !== ASK.recommended ||
      ask.reason !== ASK.reason
    )
      failures.push(`the row's ask is ${JSON.stringify(ask)}, not what the agent asked`)
    // The row waits on the person, though the window does not follow it.
    const waiting = await run.page
      .locator(
        `.workspace-list [data-list-group="needs-you"] [data-session-row="${conversationId}"]`,
      )
      .waitFor({ timeout: 15_000 })
      .then(() => true)
      .catch(() => false)
    if (!waiting)
      failures.push("the asked conversation's row does not wait on the person")
    const notice = await waitFor(async () => {
      const notices = await hostNotices(run.page)
      return (
        notices?.up.find(
          (each) =>
            each.conversationId === conversationId &&
            each.executionId === ask.askId &&
            each.permissionId === "owner-ask",
        ) ?? null
      )
    }, 30_000)
    if (!notice) {
      failures.push("no notice was posted for the agent's ask")
      return
    }
    if (notice.title !== ASK.question)
      failures.push(
        `the ask's notice says ${JSON.stringify(notice.title)}, not the question`,
      )
    if (notice.body !== title)
      failures.push(
        `the ask's notice names ${JSON.stringify(notice.body)}, not the conversation ${JSON.stringify(title)}`,
      )
    // Nothing was taken from the person: the window shows what it showed.
    const still = await run.page
      .locator(`${selectorFor.sessionListRowFor(run.main.id)}[aria-selected=true]`)
      .count()
    if (still !== shown)
      failures.push("the window left the conversation it showed when the agent asked")
    const listening = await run.page.evaluate(
      (id) => window.__fakeHostOpenNotice?.(id) ?? 0,
      conversationId,
    )
    if (listening !== 1)
      failures.push(`${listening} windows listened for an opened notice, not 1`)
    const card = run.page.locator(`[data-owner-ask="${ask.askId}"]`)
    const drawn = await card
      .waitFor({ timeout: 15_000 })
      .then(() => true)
      .catch(() => false)
    if (!drawn) {
      failures.push("opening the ask's notice did not bring up its card")
      return
    }
    const recommended = card.locator(
      `[data-owner-ask-choice="${ASK.choices[ASK.recommended].label}"] [data-owner-ask-recommended]`,
    )
    if (
      (await card.locator("[data-owner-ask-recommended]").count()) !== 1 ||
      !(await recommended.count())
    )
      failures.push("the card does not mark the one recommended choice")
    if (!(await card.getByText(ASK.reason, { exact: false }).count()))
      failures.push("the card does not say why that choice is recommended")
    for (const choice of ASK.choices)
      if (!(await card.getByText(choice.consequence).count()))
        failures.push(`the card does not say what ${JSON.stringify(choice.label)} does`)
    // The person picks the other choice: its label goes as their message.
    const other = ASK.choices.find((_, index) => index !== ASK.recommended).label
    await card.locator(`[data-owner-ask-choice="${other}"]`).click()
    await settled(
      run,
      conversationId,
      new Set(asked?.messages.map((each) => each.executionId) ?? []),
      other,
      "completed",
      failures,
    )
    await expectShown(run.page, `${SAYS.answered}${other}`, failures)
    const gone = await card
      .waitFor({ state: "detached", timeout: 15_000 })
      .then(() => true)
      .catch(() => false)
    if (!gone) failures.push("the ask's card stayed after the person answered")
    if (
      await waitFor(async () => (await rowAsk()) === undefined, 15_000).then(
        (cleared) => !cleared,
      )
    )
      failures.push("the list row still carries the ask the person answered")
    await noticeTakenBack(run, notice, "answered with a choice", failures)
    await openStressConversation(run.page, run.main.id, run.main.title)
  },

  async accessRequest(run, step, failures) {
    const { agents } = await run.client.agents.list()
    const conversationId = randomUUID()
    run.checked.push(conversationId)
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    const naming = prompt(run, STRESS.reply, step, " access")
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, naming),
    )
    const { view: named } = await settled(
      run,
      conversationId,
      new Set(),
      naming,
      "completed",
      failures,
    )
    if (failures.length) return
    // Auto is what the agent asks for. The stress stack's agent offers it;
    // one that does not would check nothing here, so that is a failure, not
    // a pass.
    const offered =
      agents
        .flatMap((agent) => agent.models)
        .find((model) => model.modelId === named?.model)
        ?.approvalModes?.map((choice) => choice.id) ?? []
    if (!offered.includes(ACCESS.access)) {
      failures.push(
        `${named?.model} offers no ${ACCESS.access}, so the access request was not checked`,
      )
      return
    }
    try {
      await approveAccess(run, step, conversationId, named, failures)
    } finally {
      // Leave the chat as other steps expect it, however this one ended.
      await run.client.conversation.setApprovalMode(conversationId, "ask").catch(() => {})
      await openStressConversation(run.page, run.main.id, run.main.title)
    }
  },

  async park(run, step, failures) {
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " park")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    const { view: before } = await settled(
      run,
      conversationId,
      new Set(),
      first,
      "completed",
      failures,
    )
    if (failures.length) return
    // Nothing reads it meanwhile: a read opens its agent, a follower keeps it.
    await sleep((PARK_IDLE_SECONDS + 4) * 1000)
    const parked = await followedOnce(run, conversationId)
    const lease = parked.lease
    if (lease?.state !== "ended" || lease?.cause !== "idle") {
      failures.push(
        `the idle conversation was not parked: its lease is ${lease?.state ?? "missing"}${
          lease?.cause ? ` (${lease.cause})` : ""
        }`,
      )
      return
    }
    // A person opens the parked conversation and writes in it: the window
    // shows it with no agent running, and Send starts one.
    await openStressConversation(
      run.page,
      conversationId,
      await titleOf(run, conversationId),
    )
    const second = prompt(run, STRESS.reply, step, " after parking")
    const seen = await send(
      run,
      conversationId,
      second,
      failures,
      new Set(before.messages.map((each) => each.executionId)),
    )
    const { view: after } = await settled(
      run,
      conversationId,
      seen,
      second,
      "completed",
      failures,
    )
    await expectShown(run.page, `${SAYS.echo}${second}`, failures)
    if (after?.lease?.state !== "live")
      failures.push(`the next message left the lease ${after?.lease?.state ?? "missing"}`)
    else if (!(after.lease.revision > (before.lease?.revision ?? 0)))
      failures.push("the next message ran on the parked agent's lease, not a new one")
    await openStressConversation(run.page, run.main.id, run.main.title)
  },

  /**
   * How agents talk, set in the default channel's settings (the Nessa
   * level): Threads take the person's own words, saved only on Save. A chat
   * started after it is told those words; the main chat, started before,
   * keeps Nessa's plain style, so its agent is not restarted by the change.
   * Only Claude's reply says its system prompt, so only there are the words
   * checked in what the agent was told.
   */
  async style(run, step, failures) {
    const marker = `Style marker r${step.index} s${run.knobs.seed}: answer in one line.`
    const told = run.stack.agent === "claude"
    const close = async () => {
      const dialog = run.page.locator(css.channelSettingsDialog)
      for (let tries = 0; tries < 3 && (await dialog.count()) > 0; tries += 1) {
        await run.page.keyboard.press(keys.escape)
        await dialog.waitFor({ state: "detached", timeout: 2_000 }).catch(() => {})
      }
    }
    try {
      await run.page
        .locator(`${css.sidebar} nav`)
        .getByText(MAIN_CHANNEL, { exact: true })
        .first()
        .hover({ timeout: 15_000 })
      await run.page
        .locator(
          `${css.channelSettingsOpen}[data-channel-settings-open="gateway-conversations"]`,
        )
        .click({ timeout: 10_000 })
      const control = run.page
        .locator(css.channelSettingsDialog)
        .locator('[data-reply-style="nessa"]')
      const threads = control.locator('[data-reply-style-role="thread"]')
      await threads.locator("[data-choice-trigger]:enabled").waitFor({ timeout: 10_000 })
      await pickChoice(run.page, threads, "custom")
      await control.locator('[data-reply-style-text="thread"]').fill(`  ${marker}  `)
      // Choosing "Your own" saves nothing until Save.
      const before = await run.client.replyStyle.read()
      if (before.nessa.thread?.mode === "custom")
        failures.push("choosing Your own saved before Save was pressed")
      await control.locator('[data-reply-style-save="thread"]').click({ timeout: 10_000 })
      const saved = await waitFor(async () => {
        const read = await run.client.replyStyle.read()
        return (
          read.effective.thread.mode === "custom" && read.effective.thread.text === marker
        )
      }, 10_000)
      if (!saved)
        failures.push("the gateway never read back the Threads style as saved, trimmed")
      if (run.evidence)
        await run.page.screenshot({
          path: join(run.evidence, `style-${run.engine}-threads-own.png`),
        })
    } finally {
      await close()
    }
    // A new chat in the default channel is told the words.
    const text = prompt(run, STRESS.channel, step, " styled")
    await run.page.keyboard.press(keys.newSession)
    const home = run.page.locator(`${css.focusedPane} ${css.paneHome}`)
    await home.locator(css.field).waitFor({ timeout: 15_000 })
    await home.locator(css.field).fill(text)
    await home.getByRole(...composerControls.send).click({ timeout: 10_000 })
    const conversationId = await waitFor(
      () => selectedStressConversation(run.page, text),
      30_000,
    )
    if (!conversationId) {
      failures.push("the chat started after the style change was never listed")
    } else {
      run.checked.push(conversationId)
      const { view } = await settled(
        run,
        conversationId,
        new Set(),
        text,
        "completed",
        failures,
      )
      const reply = (view?.messages.find((each) => each.userText === text)?.parts ?? [])
        .filter((part) => part.kind === "text")
        .map((part) => part.text)
        .join("")
      if (told && !reply.includes(marker))
        failures.push(
          `the new chat was not told the Threads style: ${fold(reply.slice(0, 300))}`,
        )
      if (told && reply.includes("How to write to the person:"))
        failures.push("the new chat was told Nessa's plain style beside the person's own")
    }
    // The main chat started before: it keeps the plain style it began with.
    await openStressConversation(run.page, run.main.id, run.main.title)
    const again = prompt(run, STRESS.channel, step, " kept")
    const seen = await send(run, run.main.id, again, failures)
    const { view } = await settled(run, run.main.id, seen, again, "completed", failures)
    const kept = (view?.messages.find((each) => each.userText === again)?.parts ?? [])
      .filter((part) => part.kind === "text")
      .map((part) => part.text)
      .join("")
    if (told && kept.includes(marker))
      failures.push("a chat started before the change was told the new style")
    if (told && !kept.includes("How to write to the person:"))
      failures.push(
        `the main chat lost the plain style it began with: ${fold(kept.slice(0, 300))}`,
      )
    // Back to Nessa's plain style, as nothing chosen, for the steps after.
    const reset = await run.client.replyStyle.set("thread", undefined)
    if (reset.nessa.thread !== undefined || reset.effective.thread.mode !== "plain")
      failures.push(
        `clearing the Threads style read back ${JSON.stringify(reset.nessa.thread)}`,
      )
  },

  /**
   * A channel's coordinator, switched in its settings at any time. Made with
   * it on, the channel's first chat is its coordinator, and (said only in
   * Claude's reply) it is told its job. Off, the channel keeps that chat but
   * no longer opens at it: a new chat there is an ordinary thread, told no
   * job. On again, it is the same coordinator chat, pinned as the first row of
   * the channel's list above newer threads. A channel made with it
   * off says so, and still names the main chat it would have.
   */
  async coordinator(run, step, failures) {
    const told = run.stack.agent === "claude"
    const job = "You are this channel's coordinator."
    const replyTo = (view, text) =>
      (view?.messages.find((each) => each.userText === text)?.parts ?? [])
        .filter((part) => part.kind === "text")
        .map((part) => part.text)
        .join("")
    const listedNow = async (channelId) =>
      (await run.client.channels.list()).channels.find(
        (each) => each.channelId === channelId,
      )
    const dialog = run.page.locator(css.channelSettingsDialog)
    const close = async () => {
      for (let tries = 0; tries < 3 && (await dialog.count()) > 0; tries += 1) {
        await run.page.keyboard.press(keys.escape)
        await dialog.waitFor({ state: "detached", timeout: 2_000 }).catch(() => {})
      }
    }
    const switchTo = async (made, on) => {
      try {
        await run.page
          .locator(`${css.sidebar} nav`)
          .getByText(made.name, { exact: true })
          .first()
          .hover({ timeout: 15_000 })
        await run.page
          .locator(
            `${css.channelSettingsOpen}[data-channel-settings-open="${made.channelId}"]`,
          )
          .click({ timeout: 10_000 })
        const control = dialog.locator(`[data-channel-coordinator="${made.channelId}"]`)
        await control
          .locator("[data-choice-trigger]:enabled")
          .waitFor({ timeout: 10_000 })
        await pickChoice(run.page, control, on ? "on" : "off")
        const shown = await waitFor(
          async () => (await control.getAttribute("data-coordinator-on")) === String(on),
          10_000,
        )
        if (!shown)
          failures.push(
            `the channel settings never showed the coordinator ${on ? "on" : "off"}`,
          )
        if (run.evidence)
          await run.page.screenshot({
            path: join(
              run.evidence,
              `coordinator-${run.engine}-${on ? "on" : "off"}.png`,
            ),
          })
      } finally {
        await close()
      }
      const now = await waitFor(async () => {
        const listed = await listedNow(made.channelId)
        return listed?.coordinatorOn === on ? listed : null
      }, 10_000)
      if (!now)
        failures.push(`the gateway never listed the coordinator ${on ? "on" : "off"}`)
      else if (now.coordinatorConversationId !== made.coordinatorConversationId)
        failures.push(
          `switching the coordinator ${on ? "on" : "off"} changed its chat from ${made.coordinatorConversationId} to ${now.coordinatorConversationId}`,
        )
      return now
    }
    // The list's first row, and its group, while the channel is shown.
    const pinnedCheck = async (made, on) => {
      await run.page
        .locator(`${css.sidebar} nav`)
        .getByText(made.name, { exact: true })
        .first()
        .click({ timeout: 15_000 })
      const top = async () => {
        const row = run.page.locator("[data-list-group] [data-session-row]").first()
        if ((await row.count()) === 0) return null
        return {
          id: await row.getAttribute("data-session-row"),
          group: await row.evaluate((node) =>
            node.closest("[data-list-group]")?.getAttribute("data-list-group"),
          ),
        }
      }
      const pinned = await waitFor(async () => {
        const first = await top()
        const at =
          first?.id === made.coordinatorConversationId && first.group === "coordinator"
        return at === on ? first : null
      }, 10_000)
      if (!pinned)
        failures.push(
          on
            ? `the coordinator is not the list's first row, in its own group: ${JSON.stringify(await top())}`
            : `the list still pins the coordinator while it is off`,
        )
    }
    try {
      const made = await makeChannel(run, failures, {
        name: `stress-coord-r${step.index}-s${run.knobs.seed}`,
      })
      if (made) {
        // Made with it on, the channel opens at its coordinator's chat.
        const first = await talkInChannel(run, step, made, failures)
        if (first && first !== made.coordinatorConversationId)
          failures.push(
            `the first chat in a channel with a coordinator is ${first}, not its coordinator ${made.coordinatorConversationId}`,
          )
        if (first && told) {
          const view = await run.client.conversation.read(first)
          const reply = (view?.messages ?? [])
            .flatMap((each) => each.parts ?? [])
            .filter((part) => part.kind === "text")
            .map((part) => part.text)
            .join("")
          if (!reply.includes(job))
            failures.push(
              `the coordinator was not told its job: ${fold(reply.slice(0, 300))}`,
            )
        }
        // Off: a new chat in the channel is an ordinary thread, told no job.
        if (await switchTo(made, false)) {
          const text = prompt(run, STRESS.channel, step, " without a coordinator")
          await run.page.keyboard.press(keys.newSession)
          const home = run.page.locator(`${css.focusedPane} ${css.paneHome}`)
          await home.locator(css.field).waitFor({ timeout: 15_000 })
          await home.locator(css.field).fill(text)
          await home.getByRole(...composerControls.send).click({ timeout: 10_000 })
          const thread = await waitFor(
            () => selectedStressConversation(run.page, text),
            30_000,
          )
          if (!thread)
            failures.push("the chat started with the coordinator off was never listed")
          else {
            run.checked.push(thread)
            if (thread === made.coordinatorConversationId)
              failures.push(
                "a new chat with the coordinator off opened the coordinator's chat",
              )
            const { view } = await settled(
              run,
              thread,
              new Set(),
              text,
              "completed",
              failures,
            )
            const { conversations } = await run.client.conversation.list({})
            const summary = conversations.find((each) => each.conversationId === thread)
            if (summary?.channelId !== made.channelId)
              failures.push(
                `the chat started with the coordinator off is in ${summary?.channelId ?? "no channel"}`,
              )
            if (told && replyTo(view, text).includes(job))
              failures.push(
                "a thread started with the coordinator off was told the coordinator's job",
              )
          }
          // Off, it is an ordinary row: no Coordinator group in the list.
          await pinnedCheck(made, false)
          // On again: the same coordinator chat, which still answers.
          if (await switchTo(made, true)) {
            // Pinned at the top of the list, above the newer thread.
            await pinnedCheck(made, true)
            await openStressConversation(run.page, made.coordinatorConversationId)
            const again = prompt(run, STRESS.channel, step, " coordinator again")
            const seen = await send(run, made.coordinatorConversationId, again, failures)
            await settled(
              run,
              made.coordinatorConversationId,
              seen,
              again,
              "completed",
              failures,
            )
          }
        }
      }
      // Made with it off: it says so and still names the chat it would have.
      const quiet = await makeChannel(run, failures, {
        name: `stress-quiet-r${step.index}-s${run.knobs.seed}`,
        coordinator: false,
      })
      if (quiet && !quiet.coordinatorConversationId)
        failures.push(
          "a channel made without a coordinator names no main chat to turn on",
        )
    } finally {
      await run.page
        .locator(`${css.sidebar} nav`)
        .getByText("Conversations", { exact: true })
        .first()
        .click({ timeout: 10_000 })
        .catch(() => {})
      await openStressConversation(run.page, run.main.id, run.main.title)
    }
  },

  async huddle(run, step, failures) {
    const conversationId = randomUUID()
    run.failureConversationId = conversationId
    const first = prompt(run, STRESS.huddle, step)
    const page = run.page
    const shot = (what) =>
      run.evidence
        ? page
            .screenshot({ path: join(run.evidence, `huddle-${what}.png`) })
            .catch(() => {})
        : undefined
    await run.exclusive(async () => {
      // Drain background work before starting this app fixture. Otherwise its
      // three-second idle lease can expire while exclusive waits (#1327).
      await run.client.conversation.create({ conversationId, agent: run.stack.agent })
      run.checked.push(conversationId)
      await run.client.conversation.submit(
        run.client.conversation.prepare(conversationId, first),
      )
      const { view } = await settled(
        run,
        conversationId,
        new Set(),
        first,
        "completed",
        failures,
      )
      if (!view) return
      await withStepFailureEvidence(
        async () => {
          await openStressConversation(
            page,
            conversationId,
            await titleOf(run, conversationId),
          )
          await talkBoard(run, step, conversationId, failures, shot)
        },
        {
          failures,
          capture: () => shot(`${run.engine}-${step.index}-failed-before-cleanup`),
          cleanup: () => openStressConversation(page, run.main.id, run.main.title),
        },
      )
    })
  },

  async provenance(run, step, failures) {
    // A thread the parent did not create: the parent's close of it is refused.
    const strangerId = randomUUID()
    await run.client.conversation.create({
      conversationId: strangerId,
      agent: run.stack.agent,
    })
    run.checked.push(strangerId)
    const text = prompt(
      run,
      STRESS.provenance,
      step,
      ` args:${JSON.stringify({ threadId: strangerId })} :args`,
    )
    const seen = await send(run, run.main.id, text, failures)
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    if (!turn) return
    const said = (turn.parts ?? []).map((part) => part.text ?? "").join("")
    const found = new RegExp(
      `${SAYS.provenance}([0-9a-f-]{36}) \\| (.*?) \\| (.*?) \\| (.*)`,
      "s",
    ).exec(said)
    if (!found) {
      failures.push(
        `the provenance turn did not say what happened: ${said.slice(0, 200)}`,
      )
      return
    }
    const [, childId, refused, closing, reply] = found
    run.checked.push(childId)
    // The thread read the message back to the person's exact words: the turn
    // the check typed, not the parent agent's relay of it, sent from the
    // window, which signs in with a token on this computer.
    if (!reply.includes(`${SAYS.origin}person on computer: ${text}`))
      failures.push(
        `the thread did not follow its message back to the person's words: ${reply.slice(0, 200)}`,
      )
    // Asked for, the person's earlier words in the main chat come with it:
    // each the person's, none the turn the chain starts at.
    const before = reply.split(SAYS.earlier)[1]
    let earlier
    try {
      earlier = before === "${chain.earlier}" ? [] : JSON.parse(before)
    } catch {
      earlier = null
    }
    if (
      !Array.isArray(earlier) ||
      earlier.some((each) => each.origin !== "person" || each.said === text)
    )
      failures.push(
        `the thread's read of the person's earlier words was wrong: ${before?.slice(0, 200)}`,
      )
    else log(`provenance earlier ${earlier.length}`)
    if (!refused.startsWith("not_your_thread"))
      failures.push(
        `closing a thread the parent did not create was not refused: ${refused}`,
      )
    if (closing !== "true")
      failures.push(`the parent's close of its own thread answered closing ${closing}`)
    // The closed thread's agent is parked; the stranger, never closed, is not.
    const parked = await waitFor(async () => {
      const lease = (await followedOnce(run, childId)).lease
      return lease?.state === "ended" && lease?.cause === "idle" ? lease : null
    }, 45_000)
    if (!parked) {
      const lease = (await followedOnce(run, childId)).lease
      failures.push(
        `the thread its parent closed was not parked: its lease is ${lease?.state ?? "missing"}`,
      )
    }
  },

  async close(run, step, failures) {
    const conversationId = randomUUID()
    run.failureConversationId = conversationId
    const parkingOffset = run.stack.gateway.log().length
    const first = prompt(run, STRESS.reply, step, " close")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    run.checked.push(conversationId)
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    const { view: before } = await settled(
      run,
      conversationId,
      new Set(),
      first,
      "completed",
      failures,
    )
    if (failures.length) return
    // The window shows it, so it is followed: an agent that closed its own
    // thread is parked anyway, unlike one that is only idle.
    await openStressConversation(
      run.page,
      conversationId,
      await titleOf(run, conversationId),
    )
    const closing = prompt(run, STRESS.close, step)
    const seen = await send(
      run,
      conversationId,
      closing,
      failures,
      new Set(before.messages.map((each) => each.executionId)),
    )
    const { view: closed } = await settled(
      run,
      conversationId,
      seen,
      closing,
      "completed",
      failures,
    )
    if (!closed) return
    let acceptance
    try {
      acceptance = closeAcceptance(
        readFileSync(run.acpRecording, "utf8"),
        closing,
        conversationId,
      )
    } catch {
      acceptance = { state: "invalid-recording", threadId: null, closing: null }
    }
    log(`close_thread acceptance ${JSON.stringify({ conversationId, ...acceptance })}`)
    if (acceptance.state !== "accepted") {
      failures.push(`close_thread did not accept this thread: ${acceptance.state}`)
      log(
        `close_thread parking ${JSON.stringify(parkingEvidence(run.stack.gateway.log(), parkingOffset, conversationId))}`,
      )
      return
    }
    await expectShown(run.page, SAYS.closed, failures)
    // The parking sweep looks at least every 15 s. The read that saw the
    // turn settle may have opened it again after a first park; the thread
    // is still closed, so that opening is parked too.
    const parked = await waitFor(async () => {
      const lease = (await followedOnce(run, conversationId)).lease
      return lease?.state === "ended" && lease?.cause === "idle" ? lease : null
    }, 45_000)
    log(
      `close_thread parking ${JSON.stringify({ conversationId, ...parkingEvidence(run.stack.gateway.log(), parkingOffset, conversationId) })}`,
    )
    if (!parked) {
      const lease = (await followedOnce(run, conversationId)).lease
      failures.push(
        `the closed thread's agent was not parked: its lease is ${lease?.state ?? "missing"}${
          lease?.cause ? ` (${lease.cause})` : ""
        }`,
      )
      return
    }
    // The gateway log carries what the conversation service said about this
    // thread. Those lines come from a crate split out of the gateway, and a
    // default log filter that named only the server crate dropped them all.
    const said = parkingEvidence(run.stack.gateway.log(), parkingOffset, conversationId)
    if (!said.accepted || said.targetParking === 0)
      failures.push(
        `the gateway log does not carry the conversation service's lines for this thread: ${JSON.stringify(said)}`,
      )
    // A read opens its agent, as a read does, and the thread is still
    // closed: that agent is parked again once idle, though the window
    // follows it. Only a message opens the thread.
    await run.client.conversation.read(conversationId)
    const again = await waitFor(async () => {
      const lease = (await followedOnce(run, conversationId)).lease
      return lease?.state === "ended" &&
        lease?.cause === "idle" &&
        lease.revision > parked.revision
        ? lease
        : null
    }, 45_000)
    log(
      `close_thread reopened-read parking ${JSON.stringify({ conversationId, ...parkingEvidence(run.stack.gateway.log(), parkingOffset, conversationId) })}`,
    )
    if (!again) {
      const lease = (await followedOnce(run, conversationId)).lease
      failures.push(
        `the closed thread a read opened was not parked again: its lease is ${
          lease?.state ?? "missing"
        }${lease?.cause ? ` (${lease.cause})` : ""}, revision ${lease?.revision} after ${
          parked.revision
        }`,
      )
      return
    }
    const next = prompt(run, STRESS.reply, step, " after close_thread")
    const after = await send(
      run,
      conversationId,
      next,
      failures,
      new Set(closed.messages.map((each) => each.executionId)),
    )
    const { view: reopened } = await settled(
      run,
      conversationId,
      after,
      next,
      "completed",
      failures,
    )
    await expectShown(run.page, `${SAYS.echo}${next}`, failures)
    if (reopened?.lease?.state !== "live")
      failures.push(
        `the message after close_thread left the lease ${reopened?.lease?.state ?? "missing"}`,
      )
    else if (!leaseReopenedAfter(reopened.lease, again))
      failures.push("the message after close_thread ran on the closed agent's lease")
    await openStressConversation(run.page, run.main.id, run.main.title)
  },

  async rename(run, step, failures) {
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " rename")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    run.checked.push(conversationId)
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    const { view: before } = await settled(
      run,
      conversationId,
      new Set(),
      first,
      "completed",
      failures,
    )
    if (failures.length) return
    const renamed = `Renamed s${run.knobs.seed} ${step.index}`
    // As a person does: the row's More menu under the pointer, Rename, the
    // new title typed over the old one, and Return.
    await choose(run.page, conversationId, "Rename")
    const field = run.page.locator("[data-session-rename]")
    await field.fill(renamed)
    await field.press("Enter")
    const listed = await waitFor(async () => {
      const { conversations } = await run.client.conversation.list({})
      return (
        conversations.find((each) => each.conversationId === conversationId)?.title ===
        renamed
      )
    }, 15_000)
    if (!listed) failures.push("the gateway's list never said the new title")
    await rowShown(run.page, conversationId, renamed, failures)
    // A message after the rename keeps the person's title, not the first message's.
    await openStressConversation(run.page, conversationId, renamed)
    const second = prompt(run, STRESS.reply, step, " after rename")
    const seen = await send(
      run,
      conversationId,
      second,
      failures,
      new Set(before.messages.map((each) => each.executionId)),
    )
    await settled(run, conversationId, seen, second, "completed", failures)
    if ((await titleOf(run, conversationId)) !== renamed)
      failures.push("a message after the rename changed the title back")
    await openStressConversation(run.page, run.main.id, run.main.title)
  },

  /**
   * A conversation switched to the other agent from the composer's picker,
   * as a person does: the pick is only staged (taken back once, then picked
   * again), and the next message sent makes the switch. Then forked from
   * its row's More menu. Each is talked
   * to after: the first turn of each new session is the one given the
   * earlier turns (`SAYS.carried`), and it runs on the agent and model
   * asked for. The window draws a line at the switch, and the fork opens
   * on the copied turns and a line after them.
   */
  /**
   * A switch to an agent that is not signed in where the conversation runs:
   * the harness answers its session/new with "authentication required"
   * (`--refusals`), as Codex on a VM with no saved login does. The send is
   * refused before anything reaches either agent, the picker says why in
   * words (not a bare model_change_not_applied) and the composer nothing, the picker goes back to the
   * conversation's own model, no switch is recorded, and the next message
   * runs on the agent it was on.
   */
  async unsigned(run, step, failures) {
    try {
      await unsignedSwitch(run, step, failures)
    } finally {
      // The steps after read the main conversation in the window.
      await openStressConversation(run.page, run.main.id, run.main.title)
    }
  },
  async unsignedSwitchBody(run, step, failures) {
    const other = otherAgent(run.stack.agent)
    const target = catalog.models.find((model) => model.modelId === MODELS[other])
    if (!target) throw new CannotRun(`the catalog has no ${MODELS[other]}`)
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " unsigned")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    run.checked.push(conversationId)
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    const { view: before } = await settled(
      run,
      conversationId,
      new Set(),
      first,
      "completed",
      failures,
    )
    if (failures.length || !before) return
    await openStressConversation(
      run.page,
      conversationId,
      await titleOf(run, conversationId),
    )
    const picker = dock(run.page).locator(css.modelChip)
    const own = catalog.models.find((model) => model.modelId === before.model)
    if (!own) throw new CannotRun(`the catalog has no ${before.model}`)
    const refusal = join(run.stack.refusals, `${other}.session-new`)
    writeFileSync(refusal, "")
    try {
      await pickModel(run.page, picker, target)
      const refused = prompt(run, STRESS.reply, step, " refused switch")
      await send(run, conversationId, refused, failures)
      const pickerSays = () =>
        dock(run.page)
          .locator(composerControls.pickerNote)
          .getAttribute("data-tooltip")
          .then((text) => text ?? "")
          .catch(() => "")
      const said = await waitFor(
        async () => (await pickerSays()).includes(composerControls.signInRequired),
        30_000,
      )
      await composerSaysNothing(run.page, "of the refused switch", failures)
      if (run.evidence)
        await dock(run.page)
          .locator(".desktop-composer")
          .screenshot({ path: join(run.evidence, `unsigned-${run.engine}.png`) })
          .catch(() => {})
      if (!said) {
        failures.push(
          `the picker did not say the agent isn't signed in: ${JSON.stringify(await pickerSays())}`,
        )
      }
      if (run.evidence)
        await run.page.screenshot({
          path: join(run.evidence, `unsigned-${run.engine}.png`),
          scale: "css",
        })
      if (
        !(await waitFor(
          async () =>
            (await picker.getAttribute("aria-label"))?.includes(own.displayName),
          5_000,
        ))
      )
        failures.push("the picker did not go back to the conversation's own model")
      const view = await run.client.conversation.read(conversationId)
      if (view.model !== before.model || view.handoffs.length)
        failures.push("a switch the agent could not open was recorded")
      if (view.messages.some((each) => each.userText === refused))
        failures.push("the message sent with the refused switch reached an agent")
    } finally {
      rmSync(refusal, { force: true })
    }
    const after = prompt(run, STRESS.reply, step, " after refused switch")
    const seen = await send(run, conversationId, after, failures)
    const { turn } = await settled(
      run,
      conversationId,
      seen,
      after,
      "completed",
      failures,
    )
    if (turn && turn.model && turn.model !== before.model)
      failures.push(`the next message ran on ${turn.model}, not ${before.model}`)
  },
  /**
   * A model change the agent fails while a turn runs: the harness answers
   * its set_config_option with an internal error (`--refusals`), which ends
   * the agent's worker and the turn with it, as Claude's adapter did for a
   * real person. The change is settled without anyone asking again: nothing
   * stays pending, the composer says nothing of it, the turn is not
   * left at work with a Stop that stops nothing, and the next message
   * completes.
   */
  async failedControl(run, step, failures) {
    try {
      await failedModelControl(run, step, failures)
    } finally {
      // The steps after read the main conversation in the window.
      await openStressConversation(run.page, run.main.id, run.main.title)
    }
  },
  async failedControlBody(run, step, failures) {
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " control")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    run.checked.push(conversationId)
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    const { view: before } = await settled(
      run,
      conversationId,
      new Set(),
      first,
      "completed",
      failures,
    )
    if (failures.length || !before) return
    const current = catalog.models.find(
      (model) => model.modelId === before.runtime?.model,
    )
    const target =
      current &&
      catalog.models.find(
        (model) =>
          model.provider === current.provider && model.modelId !== current.modelId,
      )
    if (!target)
      throw new CannotRun("no second catalog model in this conversation's provider")
    await openStressConversation(
      run.page,
      conversationId,
      await titleOf(run, conversationId),
    )
    // Watched as a follower does, which opens and recovers nothing (a read
    // would: an opening read settles a pending change itself, and would
    // hide a gateway that never settles it on its own).
    // A follower subscribes again from its last cursor when its view ends
    // (the agent it followed was retired), as the product protocol says.
    let latest = null
    let cursor
    let following = null
    const follow = async () => {
      following = await run.client.subscriptions.view(
        conversationId,
        {
          view: (frame) => {
            latest = frame.view
            cursor = frame.cursor
          },
          ended: () => {
            following = null
          },
        },
        cursor ? { after: cursor } : {},
      )
    }
    await follow()
    const held = prompt(run, STRESS.pause, step, " control")
    await send(run, conversationId, held, failures)
    if (!(await stopShown(run.page)))
      failures.push("Send did not turn into Stop while the turn ran")
    const refusal = join(run.stack.refusals, `${run.stack.agent}.model`)
    writeFileSync(refusal, "")
    let answer
    try {
      answer = await run.client.conversation
        .setModel(conversationId, target.modelId)
        .then(
          () => "applied",
          (error) => error.code ?? error.message,
        )
    } finally {
      rmSync(refusal, { force: true })
    }
    if (answer === "applied")
      failures.push("a model control the agent failed was answered as applied")
    const unsettled = (view) =>
      view.modelChange || view.messages.some((each) => !ended(each))
    const quiet = await waitFor(async () => {
      if (!following) await follow().catch(() => {})
      return latest && !unsettled(latest) ? latest : null
    }, 90_000).finally(() => following?.close())
    if (!quiet) {
      const view = latest ?? { messages: [] }
      failures.push(
        `after a failed model control the conversation stayed unsettled: ${JSON.stringify(
          {
            modelChange: view.modelChange,
            turns: view.messages.map((each) => each.status),
          },
        )}`,
      )
      return
    }
    await composerSaysNothing(run.page, "after a failed model control", failures)
    if (run.evidence)
      writeFileSync(
        join(run.evidence, `failed-control-${run.engine ?? "window"}-${step.index}.json`),
        JSON.stringify(
          { answer, before: current.modelId, after: quiet.runtime?.model },
          null,
          2,
        ),
      )
    const after = prompt(run, STRESS.reply, step, " after failed control")
    const seen = await send(run, conversationId, after, failures)
    await settled(run, conversationId, seen, after, "completed", failures)
    if (failures.length) return
    await stuckModelControl(run, step, conversationId, target, failures)
  },
  /**
   * The same failed control, but the agent cannot be opened again to settle
   * it either (its `session/resume` fails too), as a person's Boat chat was
   * left: the gateway holds the change unconfirmed, and only a send can
   * settle it. The window must not wait for a settle nobody asks for: the
   * composer says nothing, Send is offered, and once the agent can open
   * again the person's next message settles the change and completes.
   */
  async stuckControlBody(run, step, conversationId, target, failures) {
    const control = join(run.stack.refusals, `${run.stack.agent}.model`)
    const resume = join(run.stack.refusals, `${run.stack.agent}.session-resume`)
    const held = prompt(run, STRESS.pause, step, " stuck control")
    await send(run, conversationId, held, failures)
    if (!(await stopShown(run.page)))
      failures.push("Send did not turn into Stop while the turn ran")
    writeFileSync(control, "")
    writeFileSync(resume, "")
    try {
      const answer = await run.client.conversation
        .setModel(conversationId, target.modelId)
        .then(
          () => "applied",
          (error) => error.code ?? error.message,
        )
      rmSync(control, { force: true })
      if (answer === "applied")
        failures.push("a model control the agent failed was answered as applied")
      const stuck = await waitFor(async () => {
        const view = await run.client.conversation.read(conversationId).catch(() => null)
        return view?.modelChange?.status === "recovery_required" &&
          view.messages.every((each) => ended(each))
          ? view
          : null
      }, 60_000)
      if (!stuck) {
        failures.push("the change the agent could not settle was not left unconfirmed")
        return
      }
      const box = dock(run.page).locator("textarea")
      await box.fill(prompt(run, STRESS.reply, step, " while unconfirmed"))
      const offered = await waitFor(
        async () =>
          await dock(run.page)
            .getByRole(...composerControls.send)
            .isEnabled()
            .catch(() => false),
        10_000,
      )
      if (!offered)
        failures.push(
          "Send was held while the gateway waited for a send to settle the change",
        )
      await composerSaysNothing(run.page, "while the change was unconfirmed", failures)
      const warns = await dock(run.page)
        .locator(composerControls.pickerNote)
        .getAttribute("data-attention")
        .catch(() => null)
      if (warns !== "warning")
        failures.push("the picker did not mark the change it will check on the next send")
      if (run.evidence)
        await dock(run.page)
          .screenshot({ path: join(run.evidence, `stuck-control-${run.engine}.png`) })
          .catch(() => {})
      await box.fill("")
    } finally {
      rmSync(control, { force: true })
      rmSync(resume, { force: true })
    }
    const next = prompt(run, STRESS.reply, step, " after stuck control")
    // Read before Send: the picker shows the settled model once the send settles it.
    const shown = await dock(run.page)
      .locator(css.modelChip)
      .getAttribute("aria-label")
      .catch(() => null)
    const seen = await send(run, conversationId, next, failures)
    const { view, turn } = await settled(
      run,
      conversationId,
      seen,
      next,
      "completed",
      failures,
    )
    const ran = catalog.models.find((model) => model.modelId === turn?.model)
    if (ran && shown && !shown.includes(ran.displayName))
      failures.push(
        `the picker showed "${shown}" and the message ran on ${ran.displayName}`,
      )
    if (view?.modelChange)
      failures.push(
        `the send did not settle the unconfirmed change: ${JSON.stringify(view.modelChange)}`,
      )
    await composerSaysNothing(run.page, "after the send settled the change", failures)
  },
  async handoff(run, step, failures) {
    const other = otherAgent(run.stack.agent)
    const target = catalog.models.find((model) => model.modelId === MODELS[other])
    if (!target) throw new CannotRun(`the catalog has no ${MODELS[other]}`)
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " handoff")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    run.checked.push(conversationId)
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    const { view: before } = await settled(
      run,
      conversationId,
      new Set(),
      first,
      "completed",
      failures,
    )
    if (failures.length || !before) return
    await openStressConversation(
      run.page,
      conversationId,
      await titleOf(run, conversationId),
    )
    // Picking another provider's model only stages it: nothing is switched,
    // and no line is drawn, until a message is sent. Picking the
    // conversation's own model again takes the pick back.
    const picker = dock(run.page).locator(css.modelChip)
    const own = catalog.models.find((model) => model.modelId === before.model)
    if (!own) throw new CannotRun(`the catalog has no ${before.model}`)
    const line = run.page.locator(`${css.handoffLine}[data-handoff="switch"]`)
    const unswitched = async (what) => {
      if (
        !(await waitFor(
          async () =>
            (await picker.getAttribute("aria-label"))?.includes(what.displayName),
          5_000,
        ))
      )
        failures.push(`the picker did not show ${what.displayName} once picked`)
      const moved = await waitFor(async () => {
        const view = await run.client.conversation.read(conversationId)
        return view.model !== before.model || view.handoffs.length > 0
      }, 2_000)
      if (moved)
        failures.push(
          `picking ${what.displayName} switched the conversation before a send`,
        )
      if (await line.count())
        failures.push(`picking ${what.displayName} drew a switch line before a send`)
    }
    await pickModel(run.page, picker, target)
    await unswitched(target)
    if (run.evidence)
      await run.page.screenshot({
        path: join(run.evidence, `handoff-${run.engine}-staged.png`),
        scale: "css",
      })
    await pickModel(run.page, picker, own)
    await unswitched(own)
    await pickModel(run.page, picker, target)
    await unswitched(target)
    if (failures.length) return
    const afterSwitch = prompt(run, STRESS.reply, step, " after switch")
    const seen = await send(
      run,
      conversationId,
      afterSwitch,
      failures,
      new Set(before.messages.map((each) => each.executionId)),
    )
    const switched = await waitFor(async () => {
      const view = await run.client.conversation.read(conversationId)
      return view.model === target.modelId &&
        view.handoffs.filter((each) => each.kind === "switch").length === 1 &&
        view.handoffs.some((each) => each.kind === "switch" && each.to?.agent === other)
        ? view
        : null
    }, 30_000)
    if (!switched) {
      failures.push(`sending did not switch the conversation to ${other} once`)
      return
    }
    if (
      !(await line
        .first()
        .waitFor({ timeout: 10_000 })
        .then(() => true)
        .catch(() => false))
    )
      failures.push("the window drew no line where the conversation was switched")
    const { turn: switchedTurn } = await settled(
      run,
      conversationId,
      seen,
      afterSwitch,
      "completed",
      failures,
    )
    carriedBy(switchedTurn, target.modelId, "the switch", first, failures)
    if (!(await carriedShown(run.page, target.modelId, afterSwitch)))
      failures.push("the window does not show the reply after the switch")
    if (run.evidence)
      await run.page.screenshot({
        path: join(run.evidence, `handoff-${run.engine}-switch.png`),
        scale: "css",
      })
    // Forked from the row: the window opens the fork, on the copied turns.
    const listed = new Set(
      (await run.client.conversation.list({})).conversations.map(
        (each) => each.conversationId,
      ),
    )
    await choose(run.page, conversationId, "Fork")
    const forkId = await waitFor(async () => {
      const { conversations } = await run.client.conversation.list({})
      return conversations.find((each) => !listed.has(each.conversationId))
        ?.conversationId
    }, 15_000)
    if (!forkId) {
      failures.push("the gateway never listed the fork")
      return
    }
    run.checked.push(forkId)
    const forked = await run.client.conversation.read(forkId)
    const copied = forked.handoffs.find((each) => each.kind === "fork")
    if (forked.model !== target.modelId)
      failures.push(`the fork runs ${forked.model}, not its source's ${target.modelId}`)
    if (!copied || (copied.turns?.length ?? 0) !== 2)
      failures.push(
        `the fork copied ${copied?.turns?.length ?? "no"} turns, not the source's 2`,
      )
    if (forked.messages.length > 0) failures.push("the fork said something of its own")
    if (
      !(await run.page
        .locator(`${css.handoffLine}[data-handoff="fork"]`)
        .first()
        .waitFor({ timeout: 10_000 })
        .then(() => true)
        .catch(() => false))
    )
      failures.push("the window did not open the fork on its copied turns")
    await expectShown(run.page, `${SAYS.echo}${first}`, failures)
    const inFork = prompt(run, STRESS.reply, step, " in fork")
    const forkSeen = await send(run, forkId, inFork, failures, new Set())
    const { turn: forkTurn } = await settled(
      run,
      forkId,
      forkSeen,
      inFork,
      "completed",
      failures,
    )
    // Its own first turn ran on the same model and was given the copied turns.
    carriedBy(forkTurn, target.modelId, "the fork", first, failures)
    if (!(await carriedShown(run.page, target.modelId, inFork)))
      failures.push("the window does not show the fork's own reply after the copied one")
    if (run.evidence)
      await run.page.screenshot({
        path: join(run.evidence, `handoff-${run.engine}-fork.png`),
        scale: "css",
      })
    // Forks into one id raced from three requests: one request sent twice at
    // once, and another request. The id keeps one fork seed, so the retry is
    // the same fork (or told to wait while the first runs), and the other
    // request is refused; the fork records its copied turns once.
    const racedId = randomUUID()
    const racedRequest = randomUUID()
    const raced = await Promise.allSettled([
      run.client.conversation.fork(conversationId, {
        conversationId: racedId,
        requestId: racedRequest,
      }),
      run.client.conversation.fork(conversationId, {
        conversationId: racedId,
        requestId: racedRequest,
      }),
      run.client.conversation.fork(conversationId, {
        conversationId: racedId,
        requestId: randomUUID(),
      }),
    ])
    if (
      raced[2].status === "fulfilled" &&
      raced.slice(0, 2).some((each) => each.status === "fulfilled")
    )
      failures.push("two requests both made the raced fork")
    if (raced.every((each) => each.status === "rejected"))
      failures.push(
        `every raced fork was refused: ${raced.map((each) => each.reason?.message).join("; ")}`,
      )
    const madeIds = raced
      .filter((each) => each.status === "fulfilled")
      .map((each) => each.value.conversationId)
    if (madeIds.some((made) => made !== racedId))
      failures.push(`a raced fork request opened ${madeIds.join(", ")}, not ${racedId}`)
    // A retry turned away while the first ran is let in once it has ended.
    const again = await run.client.conversation
      .fork(conversationId, { conversationId: racedId, requestId: racedRequest })
      .then(
        (made) => made,
        (error) => error,
      )
    const owner = raced[2].status === "fulfilled" ? "other" : "first"
    if (owner === "first" && again instanceof Error)
      failures.push(
        `a retry of the raced fork was refused after it was made: ${again.message}`,
      )
    if (
      owner === "first" &&
      !(again instanceof Error) &&
      again.conversationId !== racedId
    )
      failures.push(
        `a retry of the raced fork opened ${again.conversationId}, not ${racedId}`,
      )
    run.checked.push(racedId)
    const racedView = await run.client.conversation.read(racedId)
    const racedForks = racedView.handoffs.filter((each) => each.kind === "fork")
    if (racedForks.length !== 1 || (racedForks[0].turns?.length ?? 0) !== 2)
      failures.push(
        `the raced fork recorded ${racedForks.length} forks with ${racedForks[0]?.turns?.length ?? "no"} turns, not one with the source's 2`,
      )
    // The raced fork is a chat like any other: opened from its row and
    // talked to through the composer, its reply in the window and the view.
    await openStressConversation(run.page, racedId, racedView.title ?? "")
    if (
      !(await run.page
        .locator(`${css.handoffLine}[data-handoff="fork"]`)
        .first()
        .waitFor({ timeout: 10_000 })
        .then(() => true)
        .catch(() => false))
    )
      failures.push("the window did not open the raced fork on its copied turns")
    const inRaced = prompt(run, STRESS.reply, step, " in raced fork")
    const racedSeen = await send(run, racedId, inRaced, failures, new Set())
    const { turn: racedTurn } = await settled(
      run,
      racedId,
      racedSeen,
      inRaced,
      "completed",
      failures,
    )
    carriedBy(racedTurn, target.modelId, "the raced fork", first, failures)
    if (!(await carriedShown(run.page, target.modelId, inRaced)))
      failures.push("the window does not show the raced fork's own reply")
    // The source is as it was.
    const source = await run.client.conversation.read(conversationId)
    if (source.messages.length !== 2)
      failures.push(`the fork changed its source: ${source.messages.length} turns`)
    await openStressConversation(run.page, run.main.id, run.main.title)
  },

  async delete(run, step, failures) {
    const conversationId = randomUUID()
    const first = prompt(run, STRESS.reply, step, " delete")
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, first),
    )
    await settled(run, conversationId, new Set(), first, "completed", failures)
    if (failures.length) return
    await choose(run.page, conversationId, "Delete…")
    const asked = run.page.locator("[data-delete-session]")
    await asked.waitFor({ timeout: 10_000 })
    // Marked as the delete is confirmed, and no sooner: from here a
    // follower's stream of it ending is the delete's, and before it any
    // failure of that stream is a failure.
    run.deleted.add(conversationId)
    await asked.locator('[data-delete-action="confirm"]').click({ timeout: 10_000 })
    const gone = await waitFor(async () => {
      try {
        await run.client.conversation.read(conversationId)
        return false
      } catch (error) {
        return error?.code === "conversation_deleted"
      }
    }, 60_000)
    if (!gone) failures.push("the gateway still reads the deleted conversation")
    const { conversations } = await run.client.conversation.list({})
    if (conversations.some((each) => each.conversationId === conversationId))
      failures.push("the gateway's list still holds the deleted conversation")
    const hidden = await waitFor(
      async () => (await sessionRow(run.page, conversationId).count()) === 0,
      15_000,
    )
    if (!hidden) failures.push("the window still lists the deleted conversation")
  },

  async transcriptView(run, step, failures) {
    const pane = await chooseTranscriptView(run, "Compact", failures)
    if (failures.length) return
    const transcript = pane.locator(".workspace-transcript")
    const text = prompt(run, STRESS.trace, step, " compact")
    const seen = await send(run, run.main.id, text, failures)
    // While the calls run, the compact transcript draws no run and one live
    // row, which says only that the agent is working: no call is named.
    const live = await waitFor(
      () =>
        transcript.evaluate(
          (element, tools) => {
            const rows = [...element.querySelectorAll(".workspace-live")]
            if (rows.length !== 1) return null
            const said = rows[0].textContent ?? ""
            return {
              said,
              runs: element.querySelectorAll('[data-slot="tool-trace"]').length,
              named: tools.filter((tool) => said.includes(tool)),
            }
          },
          TRACE_CALLS.flatMap((call) => [call.tool, call.title ?? call.tool]),
        ),
      15_000,
    )
    if (!live) failures.push("the compact transcript drew no live row while the turn ran")
    else {
      if (!live.said.includes("Working"))
        failures.push(`the compact live row reads ${JSON.stringify(live.said)}`)
      if (live.named.length)
        failures.push(`the compact live row names ${JSON.stringify(live.named)}`)
      if (live.runs !== 0)
        failures.push(`the compact transcript drew ${live.runs} tool runs while working`)
      if (run.evidence) await liveShot(run, transcript, "compact-live")
    }
    const { view, turn } = await settled(
      run,
      run.main.id,
      seen,
      text,
      "completed",
      failures,
    )
    if (!turn) return
    // Compact only draws less: the gateway keeps every call.
    const kept = view.tools.filter((tool) => tool.executionId === turn.executionId)
    if (kept.length !== TRACE_CALLS.length)
      failures.push(
        `the compact turn kept ${kept.length} calls, not ${TRACE_CALLS.length}`,
      )
    const message = transcript
      .locator('.workspace-message[data-role="agent"]')
      .filter({ hasText: `${SAYS.trace}${text}` })
    const replied = await waitFor(
      async () =>
        (await message.count()) === 1 &&
        (await transcript.locator(".workspace-live").count()) === 0,
      15_000,
    )
    if (!replied) failures.push("the compact transcript does not show the reply alone")
    const runs = await transcript.locator('[data-slot="tool-trace"]').count()
    if (runs !== 0) failures.push(`the compact transcript draws ${runs} tool runs`)
    if (run.evidence)
      await transcript.screenshot({
        path: join(run.evidence, `transcript-${run.engine ?? "window"}-compact.png`),
      })
    if (failures.length) return
    // The choice is remembered: a reload draws the transcript compact again.
    await run.page.reload()
    await run.page
      .locator(selectorFor.sessionListRowFor(run.main.id))
      .waitFor({ timeout: 30_000 })
    await unlockRequestedVault(run.client, run.page)
    await openStressConversation(run.page, run.main.id, run.main.title)
    const reloaded = run.page
      .locator(css.pane)
      .filter({ has: run.page.locator(css.conversationDock) })
      .locator(".workspace-transcript")
    const remembered = await waitFor(
      async () =>
        (await reloaded
          .getAttribute("data-transcript-view", { timeout: 1_000 })
          .catch(() => null)) === "compact",
      15_000,
    )
    if (!remembered) failures.push("after a reload the transcript is not compact")
    // Detailed again draws the same turn's run, folded.
    await chooseTranscriptView(run, "Detailed", failures)
    const line = reloaded
      .locator('.workspace-message[data-role="agent"]')
      .filter({ hasText: `${SAYS.trace}${text}` })
      .locator('[data-slot="tool-trace-line"]')
    const folded = await waitFor(
      async () =>
        (await line.count()) === 1 &&
        ((await line.textContent()) ?? "").includes(`Ran ${TRACE_CALLS.length} tools`),
      15_000,
    )
    if (!folded) failures.push("Detailed, chosen again, does not draw the turn's run")
    else if (run.evidence)
      await reloaded.screenshot({
        path: join(run.evidence, `transcript-${run.engine ?? "window"}-detailed.png`),
      })
  },

  async trace(run, step, failures) {
    const text = prompt(run, STRESS.trace, step)
    const seen = await send(run, run.main.id, text, failures)
    await traceWhileLive(run, failures)
    const { view, turn } = await settled(
      run,
      run.main.id,
      seen,
      text,
      "completed",
      failures,
    )
    if (!turn) return
    // The gateway keeps every call of the turn with when it started and
    // ended, and the input it was made with.
    const tools = view.tools.filter((tool) => tool.executionId === turn.executionId)
    if (tools.length !== TRACE_CALLS.length) {
      failures.push(
        `the trace turn kept ${tools.length} calls, not ${TRACE_CALLS.length}`,
      )
      return
    }
    for (const [index, tool] of tools.entries()) {
      const call = TRACE_CALLS[index]
      const name = `call ${index + 1} (${call.tool})`
      if (tool.status !== call.status)
        failures.push(`${name} is ${tool.status}, not ${call.status}`)
      if (typeof tool.startedAtMs !== "number" || typeof tool.endedAtMs !== "number") {
        failures.push(
          `${name} kept no start or end: ${tool.startedAtMs}, ${tool.endedAtMs}`,
        )
        continue
      }
      if (tool.endedAtMs < tool.startedAtMs)
        failures.push(
          `${name} ended ${tool.startedAtMs - tool.endedAtMs} ms before it started`,
        )
      let input
      try {
        input = JSON.parse(tool.input)
      } catch {
        failures.push(`${name} kept no input: ${JSON.stringify(tool.input)}`)
        continue
      }
      // A harness reports the arguments themselves (Claude) or its own
      // record of the call, naming the tool (Codex): either way the input is
      // this call's, not another's.
      const own =
        input && typeof input === "object" && "arguments" in input && "tool" in input
          ? input.tool === call.tool && JSON.stringify(input.arguments)
          : JSON.stringify(input)
      if (own !== JSON.stringify(call.arguments))
        failures.push(
          `${name}'s input ${tool.input} is not ${JSON.stringify(call.arguments)}`,
        )
    }
    // The pause runs after the previous call's end is queued and before the
    // next call is queued. Both times are that queue time on the process
    // timeline, so a late read cannot shrink the gap. The next call starts
    // no sooner than the pause.
    for (let index = 1; index < tools.length; index += 1) {
      const gap = tools[index].startedAtMs - tools[index - 1].endedAtMs
      if (gap < TRACE_PAUSE_MS - 100)
        failures.push(
          `call ${index + 1} started ${gap} ms after the one before, not after the pause`,
        )
    }
    // The window draws the run as one line, folded once the agent wrote past
    // it, that opens to a row per call, each with its bar and status.
    const message = run.page
      .locator('.workspace-message[data-role="agent"]')
      .filter({ hasText: `${SAYS.trace}${text}` })
    const line = message.locator('[data-slot="tool-trace-line"]')
    const folded = await waitFor(
      async () =>
        (await line.count()) === 1 &&
        (await line.getAttribute("aria-expanded")) === "false" &&
        (await line.textContent()).includes(`Ran ${TRACE_CALLS.length} tools`),
      15_000,
    )
    if (run.evidence)
      await message.screenshot({
        path: join(run.evidence, `trace-${run.engine ?? "window"}-folded.png`),
      })
    if (!folded) {
      failures.push(
        `the window does not draw the run as one folded line: ${JSON.stringify(
          (await line.count()) ? await line.textContent() : null,
        )}`,
      )
      return
    }
    // The line stays where it was clicked, including when the reader is at the
    // end (`tool-disclosure.ts`). It must not move up. A close at the end may
    // move it down only as far as the browser clamps scrollTop.
    const place = (locator) =>
      locator.evaluate((element) => {
        const scroller = element.closest(".workspace-transcript")
        return {
          top: element.getBoundingClientRect().top,
          scroll: scroller instanceof HTMLElement ? scroller.scrollTop : 0,
        }
      })
    const stayed = (before, after, what) => {
      const up = before.top - after.top
      const down = after.top - before.top
      const scrollDelta = after.scroll - before.scroll
      if (up > 1) failures.push(`${what} moved up ${up.toFixed(1)}px`)
      if (down > 1 && scrollDelta > -down + 1)
        failures.push(
          `${what} moved down ${down.toFixed(1)}px while scroll moved ${scrollDelta.toFixed(1)}px`,
        )
    }
    const disclosureSettled = () =>
      run.page.waitForFunction(() => {
        const scroller = document.querySelector(".workspace-transcript")
        const moving = [
          ...(scroller?.querySelectorAll("[data-slot='tool-trace']") ?? []),
        ].some((trace) => {
          const animations =
            trace.getAnimations?.({ subtree: true }) ?? trace.getAnimations?.() ?? []
          return animations.some((animation) => animation.playState === "running")
        })
        return (
          Boolean(scroller) && !scroller.hasAttribute("data-disclosure-hold") && !moving
        )
      })
    await line.scrollIntoViewIfNeeded()
    const lineAt = await place(line)
    await line.evaluate((element) => element.click())
    await disclosureSettled()
    stayed(lineAt, await place(line), "opening the run")
    const rows = message.locator('[data-slot="tool-trace-step"]')
    const opened = await waitFor(
      async () =>
        (await line.getAttribute("aria-expanded")) === "true" &&
        (await rows.count()) === TRACE_CALLS.length,
      5_000,
    )
    if (!opened) {
      failures.push(
        `the run opened to ${await rows.count()} rows, not ${TRACE_CALLS.length}`,
      )
      return
    }
    // Each bar starts where its call started on the shared axis, as a share
    // of the run: a settled run's axis spans the first call's start to the
    // last call's end (`ToolTrace`'s domain), so a clock read wrong — the
    // wrong sign, seconds for milliseconds — moves the bars.
    const drawn = await rows.evaluateAll((elements) =>
      elements.map((row) => {
        const bar = row.querySelector('[data-slot="tool-trace-bar"]')
        const track = bar?.parentElement?.getBoundingClientRect()
        const box = bar?.getBoundingClientRect()
        return {
          status: row.dataset.status,
          at: bar && track.width ? (box.left - track.left) / track.width : null,
          width: box?.width ?? 0,
          height: box?.height ?? 0,
          over: bar && track ? box.right - track.right : 0,
          // The name column: how far its label and detail reach past the
          // start of the bar's track, and whether the label is cut.
          name: (() => {
            const column = row.children[1]
            if (!(column instanceof HTMLElement) || !track) return null
            const parts = [...column.children].filter(
              (part) => !part.classList.contains("sr-only"),
            )
            const label = parts[0]
            return {
              text: label?.textContent ?? "",
              past: Math.max(
                ...[column, ...parts].map(
                  (part) => part.getBoundingClientRect().right - track.left,
                ),
              ),
              cut: label ? label.scrollWidth > label.clientWidth : false,
              color: label ? getComputedStyle(label).color : "",
              detail: (() => {
                const detail = column.querySelector("[data-cut]")
                return detail
                  ? { text: detail.textContent ?? "", from: detail.dataset.cut }
                  : null
              })(),
            }
          })(),
        }
      }),
    )
    const first = tools[0].startedAtMs
    const span = Math.max(1, ...tools.map((tool) => tool.endedAtMs - first))
    for (const [index, row] of drawn.entries()) {
      if (row.status !== TRACE_CALLS[index].status)
        failures.push(
          `row ${index + 1} is drawn ${row.status}, not ${TRACE_CALLS[index].status}`,
        )
      const expected = (tools[index].startedAtMs - first) / span
      if (row.at === null) failures.push(`row ${index + 1} has no bar on the time axis`)
      else if (Math.abs(row.at - expected) > 0.03)
        failures.push(
          `row ${index + 1}'s bar starts ${(row.at * 100).toFixed(1)}% along the axis, not ${(expected * 100).toFixed(1)}%`,
        )
      // An instant call is still a round dot, never a hairline, and the
      // last one stays inside the axis.
      if (row.at !== null && row.width + 0.5 < row.height)
        failures.push(
          `row ${index + 1}'s bar is ${row.width.toFixed(1)} px wide, narrower than its ${row.height.toFixed(1)} px height`,
        )
      if (row.over > 0.5)
        failures.push(
          `row ${index + 1}'s bar ends ${row.over.toFixed(1)} px past the axis`,
        )
      // A name never runs under the bar: a long one is cut where the track
      // starts, and keeps its whole text for a reader.
      const title = TRACE_CALLS[index].title
      if (!row.name) failures.push(`row ${index + 1} has no name column`)
      else {
        if (row.name.past > 0.5)
          failures.push(
            `row ${index + 1}'s name runs ${row.name.past.toFixed(1)} px under the bar`,
          )
        if (title && !row.name.cut)
          failures.push(
            `row ${index + 1}'s long name ${JSON.stringify(row.name.text)} is not cut`,
          )
        if (title && row.name.text !== title)
          failures.push(
            `row ${index + 1}'s name reads ${JSON.stringify(row.name.text)}, not its title`,
          )
      }
    }
    // The verb is never cut (only a long title of its own is). A path gives
    // way from its start, keeping the file name; anything else from its end.
    // A failed call is told by its glyph and bar: its words keep the tone of
    // a call that went well (UI audit #24).
    for (const [index, row] of drawn.entries()) {
      if (!row.name) continue
      if (!TRACE_CALLS[index].title && row.name.cut)
        failures.push(`row ${index + 1}'s verb ${JSON.stringify(row.name.text)} is cut`)
      const detail = row.name.detail
      if (detail) {
        const path = !/\s/.test(detail.text) && detail.text.includes("/")
        if (detail.from !== (path ? "start" : "end"))
          failures.push(
            `row ${index + 1}'s detail ${JSON.stringify(detail.text)} is cut from its ${detail.from}`,
          )
      }
    }
    const fine = drawn.find((row) => row.status !== "failed" && row.name)
    for (const [index, row] of drawn.entries())
      if (row.status === "failed" && fine && row.name?.color !== fine.name.color)
        failures.push(
          `row ${index + 1} failed and its words are drawn ${row.name?.color}, not ${fine.name.color}`,
        )
    // A call's card shows the input it was made with.
    const asked = TRACE_CALLS.findIndex((call) => Object.keys(call.arguments).length)
    const row = rows.nth(asked)
    const rowAt = await place(row)
    await row.evaluate((element) => element.click())
    await disclosureSettled()
    stayed(rowAt, await place(row), "opening the call")
    const card = message.locator(`[id="${await row.getAttribute("aria-controls")}"]`)
    await card.getByRole("tab", { name: "Input" }).click()
    const value = String(Object.values(TRACE_CALLS[asked].arguments)[0])
    const shown = await waitFor(
      async () =>
        (
          await card.locator('[role="tabpanel"][data-state="active"]').textContent()
        )?.includes(value),
      5_000,
    )
    if (!shown) failures.push(`call ${asked + 1}'s card does not show its input ${value}`)
    if (run.evidence)
      await message.screenshot({
        path: join(run.evidence, `trace-${run.engine ?? "window"}-opened.png`),
      })
    // Folded again, as the person left it. The line does not jump up.
    const foldAt = await place(line)
    await line.evaluate((element) => element.click())
    await disclosureSettled()
    stayed(foldAt, await place(line), "closing the run")
    if (
      !(await waitFor(
        async () => (await line.getAttribute("aria-expanded")) === "false",
        5_000,
      ))
    )
      failures.push("the run does not fold again")
  },

  /**
   * The composer's row: attach, the model's short name with its thinking
   * level, and Fast at its start; at its end the access choice, a hairline,
   * then what acts. No project is offered there, and nothing stands under
   * the card.
   */
  async composer(run, step, failures) {
    await run.exclusive(async () => {
      const page = run.page
      const dock = page.locator(css.focusedPane).locator(".workspace-dock")
      const row = dock.locator(".desktop-composer-row")
      await row.waitFor({ timeout: 10_000 })
      // The chip draws the model's short name; its whole name stays its label.
      const model = await row.locator(".desktop-chip-model").evaluate((chip) => ({
        drawn:
          chip.querySelector('[data-slot="model-level-picker-trigger-label"]')
            ?.textContent ?? "",
        label: chip.getAttribute("aria-label") ?? "",
      }))
      if (model.drawn === "" || /^(Claude|OpenAI)\s/.test(model.drawn))
        failures.push(`the model chip draws ${JSON.stringify(model.drawn)}`)
      if (!model.label.includes(model.drawn))
        failures.push(
          `the model chip draws ${JSON.stringify(model.drawn)}, which its name ${JSON.stringify(model.label)} does not hold`,
        )
      if ((await dock.locator('[aria-label^="Project"]').count()) > 0)
        failures.push("the composer still offers a project")
      // Left to right: the model, Fast beside it, then at the row's end
      // access, the hairline, Send.
      const edges = await row.evaluate((element) => {
        const left = (selector) =>
          element.querySelector(selector)?.getBoundingClientRect().left ?? null
        return {
          model: left(".desktop-chip-model"),
          fast: left(".desktop-composer-fast"),
          access: left('[data-slot="composer-access-mode"]'),
          divider: left(".desktop-composer-divider"),
          send: left(".desktop-send"),
        }
      })
      const order = ["model", "fast", "access", "divider", "send"]
      if (
        order.some((part) => edges[part] === null) ||
        order.some((part, at) => at > 0 && edges[order[at - 1]] >= edges[part])
      )
        failures.push(
          `the row is not model, Fast, access, hairline, Send: ${JSON.stringify(edges)}`,
        )
      // Nothing under the card: the dock ends with the composer.
      const under = await dock.evaluate((element) => {
        const card = element.querySelector("form.desktop-composer")
        return card?.nextElementSibling?.className ?? null
      })
      if (under !== null && !String(under).includes("voice-rim"))
        failures.push(`something stands under the composer: ${JSON.stringify(under)}`)
      if (run.evidence)
        await dock.screenshot({
          path: join(run.evidence, `composer-${run.engine}.png`),
        })
      if (await modelTilesSquare(run, row, failures))
        await stackedComposersFold(run, failures)
    })
  },

  async menu(run, step, failures) {
    await run.exclusive(async () => {
      const page = run.page
      const menu = page.locator(css.menu)
      const menuGone = () => menu.waitFor({ state: "detached", timeout: 5_000 })
      // The pane's ⋯ menu: every row leads with its icon, one row height.
      await page.locator(css.paneActions).first().click()
      const pane = await menuRows(page)
      if (run.evidence)
        await page.screenshot({ path: join(run.evidence, `menu-${run.engine}-pane.png`) })
      if (pane.length === 0) failures.push("the pane's ⋯ menu showed no rows")
      for (const row of pane) {
        if (!row.leads)
          failures.push(`the pane menu's ${row.name} row leads with no icon`)
        if (row.height !== MENU_ROW)
          failures.push(
            `the pane menu's ${row.name} row is ${row.height}px, not ${MENU_ROW}`,
          )
      }
      await page.keyboard.press(keys.escape)
      await menuGone()
      // The access menu ends where the composer ends, not past it toward the
      // window's edge (UI audit #26).
      const access = page
        .locator(css.focusedPane)
        .locator('.workspace-dock [data-slot="composer-access-mode"]')
      await access.click({ timeout: 10_000 })
      const accessMenu = page.locator('[data-slot="composer-access-mode-content"]')
      await accessMenu.waitFor({ timeout: 5_000 })
      const edges = await access.evaluate((control) => ({
        menu: document
          .querySelector('[data-slot="composer-access-mode-content"]')
          ?.getBoundingClientRect().right,
        composer: control.closest(".desktop-composer")?.getBoundingClientRect().right,
      }))
      if (
        typeof edges.menu !== "number" ||
        typeof edges.composer !== "number" ||
        Math.abs(edges.menu - edges.composer) > 1
      )
        failures.push(
          `the access menu ends at ${edges.menu}, not at the composer's edge ${edges.composer}`,
        )
      if (run.evidence)
        await page.screenshot({
          path: join(run.evidence, `menu-${run.engine}-access.png`),
        })
      await page.keyboard.press(keys.escape)
      await accessMenu.waitFor({ state: "detached", timeout: 5_000 })
      // The overview's filter: the chosen choice's check before its number,
      // the numbers in one column, and a number picks its choice.
      const had = await page.evaluate((key) => {
        try {
          return JSON.parse(localStorage.getItem(key) ?? "null")
        } catch {
          return null
        }
      }, storage.overviewFilter)
      await page.keyboard.press(keys.overview)
      await page.locator(css.overviewListed).waitFor({ timeout: 10_000 })
      const filter = page.locator(css.overviewFilter)
      // The filter shows only over a list: a view that keeps every session
      // out (an idle one under Ongoing) offers Show All instead.
      if (!(await filter.isVisible())) {
        await page.getByRole("button", { name: names.overviewShowAll }).click()
        await page.locator(css.overviewListed).waitFor({ timeout: 10_000 })
      }
      // The first group starts right under the header; a row waiting on a
      // command is its title and that command, with no line of why above it;
      // a reply from the peek is the composer in its compact size (UI audit
      // #22, #23).
      const overview = await page.evaluate(() => {
        const header = document
          .querySelector(".agents-overview-header")
          ?.getBoundingClientRect()
        const group = document
          .querySelector(".agents-overview-group")
          ?.getBoundingClientRect()
        return {
          gap: header && group ? Math.round(group.top - header.bottom) : null,
          doubled: [...document.querySelectorAll(".agents-request")].filter(
            (row) =>
              row.querySelector(".agents-request-command") &&
              row.querySelector(".agents-request-why"),
          ).length,
          pills: [...document.querySelectorAll(".agents-reply-pill")].map((pill) =>
            pill.matches('.desktop-composer[data-size="compact"]'),
          ),
        }
      })
      if (overview.gap !== null && overview.gap > 32)
        failures.push(
          `the overview's first group starts ${overview.gap}px under its header`,
        )
      if (overview.doubled)
        failures.push(`${overview.doubled} overview rows say why above their command`)
      if (overview.pills.some((compact) => !compact))
        failures.push("a reply from the overview is not the composer's compact size")
      const pick = async (key) => {
        await filter.click({ timeout: 10_000 })
        await page.keyboard.press(key)
        await menuGone().catch(() =>
          failures.push(`pressing ${key} left the filter open`),
        )
      }
      await filter.click({ timeout: 10_000 })
      const rows = await menuRows(page)
      if (run.evidence)
        await page.screenshot({
          path: join(run.evidence, `menu-${run.engine}-filter.png`),
        })
      const chosen = rows.filter((row) => row.checked)
      if (chosen.length !== 2)
        failures.push(
          `the filter checked ${chosen.length} choices, not a scope and a span`,
        )
      for (const row of rows.filter((each) => each.radio)) {
        if (!row.leads) failures.push(`the filter's ${row.name} row leads with no icon`)
        if (row.shortcutRight !== rows.find((each) => each.radio)?.shortcutRight)
          failures.push(`the filter's ${row.name} number is out of the column`)
      }
      for (const row of chosen)
        if (!(row.checkRight !== null && row.checkRight <= row.shortcutLeft))
          failures.push(`the filter's ${row.name} check is not before its number`)
      await page.keyboard.press(keys.escape)
      await menuGone()
      // Any time, then Today: both keep this session (it changed just now)
      // and so the filter on the page. Each number names its span on the chip.
      for (const [key, label] of [
        [overviewFilterKeys.today, "Today"],
        [overviewFilterKeys.any, null],
      ]) {
        await pick(key)
        const named = await waitFor(async () => {
          const text = (await filter.textContent())?.trim() ?? ""
          return label ? text.endsWith(`· ${label}`) : !text.includes("·")
        }, 5_000)
        if (!named) failures.push(`pressing ${key} did not choose its span in the filter`)
      }
      // Put the filter back as the person had it, the scope last: it may
      // leave nothing listed, and the filter with it.
      const keyFor = (choice, fallback) =>
        typeof choice === "string" && Object.hasOwn(overviewFilterKeys, choice)
          ? overviewFilterKeys[choice]
          : overviewFilterKeys[fallback]
      await pick(keyFor(had?.range, "any"))
      await pick(keyFor(had?.scope, "ongoing"))
      await page.keyboard.press(keys.escape)
      await page.locator(css.overview).waitFor({ state: "detached", timeout: 10_000 })
      await openStressConversation(page, run.main.id, run.main.title)
      if (failures.length === 0) await steps.reply(run, step, failures)
    })
  },

  async paste(run, step, failures) {
    // Pasted as a copied screenshot arrives: a file and no text.
    const box = dock(run.page).locator("textarea")
    await box.click()
    await box.evaluate((field, base64) => {
      const bytes = Uint8Array.from(atob(base64), (char) => char.charCodeAt(0))
      const data = new DataTransfer()
      data.items.add(new File([bytes], "screenshot.png", { type: "image/png" }))
      field.dispatchEvent(
        new ClipboardEvent("paste", {
          clipboardData: data,
          bubbles: true,
          cancelable: true,
        }),
      )
    }, tinyPng.toString("base64"))
    const tiles = dock(run.page).locator(".desktop-composer-images img")
    if (!(await waitFor(async () => (await tiles.count()) === 1, 5000))) {
      failures.push(`the composer shows ${await tiles.count()} pasted images, not 1`)
      return
    }
    if (await box.inputValue())
      failures.push(`the paste typed ${JSON.stringify(await box.inputValue())} as well`)
    if (run.evidence)
      await dock(run.page).screenshot({
        path: join(run.evidence, `paste-${run.engine}-${step.index}-composer.png`),
      })
    const text = prompt(run, STRESS.paste, step)
    const seen = await send(run, run.main.id, text, failures)
    if (!(await waitFor(async () => (await tiles.count()) === 0, 5000)))
      failures.push("the pasted image stayed in the composer after Send")
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    if (!turn) return
    // The conversation holds one image, and the agent was handed exactly it.
    const [image, ...more] = turn.attachments ?? []
    if (!image || more.length)
      failures.push(
        `the pasted turn carries ${turn.attachments?.length ?? 0} images, not 1`,
      )
    if (!image) return
    // The very image pasted: a PNG this small is kept as it came.
    if (image.mimeType !== "image/png" || image.size !== tinyPng.length)
      failures.push(
        `the pasted turn holds ${image.mimeType} of ${image.size} bytes, not image/png of ${tinyPng.length}`,
      )
    const said = (turn.parts ?? []).map((part) => part.text ?? "").join("")
    const expected = `${SAYS.paste}${image.mimeType} ${image.size} bytes for ${text}`
    if (said !== expected)
      failures.push(
        `the agent said ${JSON.stringify(said)}, not ${JSON.stringify(expected)}`,
      )
    await expectShown(run.page, expected, failures, 10_000)
    // The window draws it on the person's message, read back from the
    // conversation once it holds the message: the held image, not the pasted
    // copy the window drew while it was on its way (`data-local-images`).
    const bubble = run.page
      .locator('.workspace-message[data-role="user"]')
      .filter({ hasText: text })
    if (
      !(await waitFor(
        async () =>
          (await bubble
            .locator('ul[aria-label="1 attachment"]:not([data-local-images]) img')
            .count()) === 1,
        10_000,
      ))
    )
      failures.push("the sent message does not show its pasted image")
    else if (run.evidence)
      await bubble.screenshot({
        path: join(run.evidence, `paste-${run.engine}-${step.index}-sent.png`),
      })
  },
  async attach(run, step, failures) {
    // Picked with the composer's Attach button, from the system's picker: a
    // file no model takes as an image, and an image.
    const notes = Buffer.from(`notes for step ${step.index}\n`)
    const attach = dock(run.page).getByRole("button", { name: "Attach files" })
    const [chooser] = await Promise.all([
      run.page.waitForEvent("filechooser", { timeout: 5000 }),
      attach.click(),
    ]).catch(() => [undefined])
    if (!chooser) {
      failures.push("the composer's Attach button opened no file picker")
      return
    }
    if (!chooser.isMultiple()) failures.push("the file picker takes one file, not many")
    await chooser.setFiles([
      { name: "notes.txt", mimeType: "text/plain", buffer: notes },
      { name: "picked.png", mimeType: "image/png", buffer: tinyPng },
    ])
    const tiles = dock(run.page).locator(".desktop-composer-image")
    const fileTile = tiles.filter({ hasText: "notes.txt" })
    if (
      !(await waitFor(
        async () =>
          (await tiles.count()) === 2 &&
          (await fileTile.count()) === 1 &&
          (await fileTile.getAttribute("data-kind")) === "file" &&
          (await fileTile.locator("img").count()) === 0 &&
          (await tiles.locator("img").count()) === 1,
        5000,
      ))
    ) {
      failures.push(
        `the composer shows ${await tiles.count()} attachments, not notes.txt by its name and the image as its picture`,
      )
      return
    }
    if (run.evidence)
      await dock(run.page).screenshot({
        path: join(run.evidence, `attach-${run.engine}-${step.index}-composer.png`),
      })
    const text = prompt(run, STRESS.attach, step)
    const seen = await send(run, run.main.id, text, failures)
    if (!(await waitFor(async () => (await tiles.count()) === 0, 5000)))
      failures.push("the attachments stayed in the composer after Send")
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    if (!turn) return
    // The conversation holds the image as an image and the file as an
    // original upload under its own name.
    const [image, ...moreImages] = turn.attachments ?? []
    const [upload, ...moreUploads] = turn.uploads ?? []
    if (!image || moreImages.length || image.mimeType !== "image/png")
      failures.push(
        `the attached turn carries ${JSON.stringify(turn.attachments ?? [])}, not one PNG`,
      )
    if (
      !upload ||
      moreUploads.length ||
      upload.name !== "notes.txt" ||
      upload.mimeType !== "text/plain" ||
      upload.size !== notes.length
    )
      failures.push(
        `the attached turn's uploads are ${JSON.stringify(turn.uploads ?? [])}, not notes.txt of ${notes.length} bytes`,
      )
    if (!image || !upload) return
    // The agent was handed the image, and found the file's very bytes where
    // its link points: the gateway placed it in the agent's workspace.
    const said = (turn.parts ?? []).map((part) => part.text ?? "").join("")
    // The link's name is the placed file's, `nessa-<digest>-notes.txt`, with
    // its punctuation escaped for the Markdown it is handed in.
    const placed = new RegExp(
      `^${escapeRegExp(`${SAYS.attach}${image.mimeType} ${image.size} bytes and `)}nessa\\\\?-[0-9a-f]{64}\\\\?-notes\\\\?\\.txt = ${escapeRegExp(JSON.stringify(notes.toString("utf8")))}${escapeRegExp(` for ${text}`)}$`,
    )
    if (!placed.test(said))
      failures.push(
        `the agent said ${JSON.stringify(said)}, not the image and notes.txt's bytes`,
      )
    // The window draws both on the person's message, read back from the
    // conversation once it holds the message, the file by its name.
    const bubble = run.page
      .locator('.workspace-message[data-role="user"]')
      .filter({ hasText: text })
    const held = bubble.locator('ul[aria-label="2 attachments"]:not([data-local-images])')
    if (
      !(await waitFor(
        async () =>
          (await held.locator("img").count()) === 1 &&
          (await held.filter({ hasText: "notes.txt" }).count()) === 1,
        10_000,
      ))
    )
      failures.push("the sent message does not show its image and notes.txt")
    else if (run.evidence)
      await bubble.screenshot({
        path: join(run.evidence, `attach-${run.engine}-${step.index}-sent.png`),
      })
  },
  async terminal(run, step, failures) {
    if (!(await terminalsBuilt(run, failures))) return
    await run.exclusive(async () => {
      const page = run.page
      const word = `stress-terminal-r${step.index}-s${run.knobs.seed}`
      const listed = await run.client.terminals.list()
      // The main chat in front, so ⌘J opens in its (default) channel and not
      // in one an earlier step left in front.
      await showMainAgain(run, failures)
      const warned = []
      const onConsole = (message) => {
        if (/terminal/i.test(message.text())) warned.push(message.text())
      }
      page.on("console", onConsole)
      // ⌘J opens a shell in the channel in front, in the focused pane.
      await page.keyboard.press(keys.newTerminal)
      const pane = page.locator(`.workspace-terminal-pane${css.focusedPane}`)
      const grid = pane.locator('[data-slot="terminal-grid"]')
      const shown = await grid.waitFor({ timeout: 15_000 }).then(
        () => true,
        () => false,
      )
      page.off("console", onConsole)
      if (!shown) {
        failures.push(
          `⌘J showed no terminal in the focused pane${warned.length ? `: ${warned.join(" | ")}` : ""}; the gateway said ${JSON.stringify(terminalLog(run))}`,
        )
        return
      }
      const opened = await waitFor(async () => {
        const now = await run.client.terminals.list()
        return now.find(
          (each) =>
            each.kind === "shell" &&
            !listed.some((old) => old.terminalId === each.terminalId),
        )
      }, 10_000)
      if (!opened) {
        failures.push("the gateway does not list the terminal ⌘J opened")
        return
      }
      // The pane sizes the terminal before anyone clicks it: a narrower
      // window reflows the shell to the pane, never shrinks its text.
      await reflowsToItsPane(page, grid, failures)
      // A real shell: what is typed runs, and its output is a row of its own.
      await grid.click()
      await page.keyboard.type(`echo ${word}`)
      await page.keyboard.press(keys.enter)
      const ran = await waitFor(
        async () => (await gridRows(grid)).some((row) => row.trim() === word),
        15_000,
      )
      if (!ran)
        failures.push(
          `the terminal never printed ${word}: ${JSON.stringify((await gridRows(grid)).filter((row) => row.trim()).slice(-4))}`,
        )
      if (run.evidence)
        await page.screenshot({
          path: join(run.evidence, `terminal-${run.engine}-${step.index}.png`),
        })
      // Listed in the channel's session list, with no path on the row.
      const row = page.locator(`[data-terminal-row="${opened.terminalId}"]`)
      if (
        !(await row.waitFor({ timeout: 5_000 }).then(
          () => true,
          () => false,
        ))
      )
        failures.push("the session list has no row for the terminal")
      else if ((await row.innerText()).includes(opened.folder))
        failures.push("the terminal's session row shows its folder")
      // Switching: a second ⌘J shell takes the pane; the first one's row
      // brings it back, its own output showing without a click in the pane.
      if (!(await switchesBack(run, grid, row, listed, opened, word)))
        failures.push(
          `the first terminal's row did not show its own output in the pane: ${JSON.stringify((await gridRows(grid)).filter((each) => each.trim()).slice(-3))}`,
        )
      // The details popover names its folder and shell.
      await pane.getByRole("button", { name: "Terminal Details" }).click()
      const details = page.locator(".workspace-terminal-details")
      const said = await details.waitFor({ timeout: 5_000 }).then(
        () => details.innerText(),
        () => "",
      )
      for (const part of ["Runs on", opened.folder, opened.shell])
        if (!said.includes(part))
          failures.push(`the terminal's details do not say ${part}`)
      await page.keyboard.press(keys.escape)
      // Renamed from its row: the gateway keeps the name and the row and the
      // pane's title show it.
      const name = `stress shell ${step.index}`
      await row.hover()
      // The row's buttons are hover chrome, hidden from the accessibility tree.
      await row.locator(`button[aria-label="Rename ${opened.title}"]`).click()
      const field = row.getByRole("textbox", { name: "Terminal name" })
      await field.fill(name)
      await field.press(keys.enter)
      const renamed = await waitFor(async () => {
        const now = await run.client.terminals.list()
        return now.find((each) => each.terminalId === opened.terminalId)?.title === name
      }, 5_000)
      if (!renamed) failures.push("the gateway did not keep the terminal's new name")
      else if (
        !(await waitFor(
          async () =>
            (await row.innerText()).includes(name) &&
            (await pane.locator(".workspace-pane-title").innerText()) === name,
          5_000,
        ))
      )
        failures.push("the terminal's row or pane does not show its new name")
      await opensBeside(run, opened, name, listed, failures)
      // The pane's × ends it: the gateway lists it no more, nor does the window.
      await pane.getByRole("button", { name: "End Terminal" }).click()
      const ended = await waitFor(async () => {
        const now = await run.client.terminals.list()
        return !now.some((each) => each.terminalId === opened.terminalId)
      }, 10_000)
      if (!ended)
        failures.push("the terminal is still listed after its pane's × ended it")
      if (
        !(await row.waitFor({ state: "detached", timeout: 5_000 }).then(
          () => true,
          () => false,
        ))
      )
        failures.push("the session list still shows the ended terminal")
      await showMainAgain(run, failures)
    })
  },
  async run(run, step, failures) {
    if (!(await terminalsBuilt(run, failures))) return
    const asked = prompt(run, STRESS.runBlock, step)
    const seen = await send(run, run.main.id, asked, failures)
    const { turn } = await settled(run, run.main.id, seen, asked, "completed", failures)
    if (!turn) return
    await run.exclusive(async () => {
      const page = run.page
      const message = page
        .locator('.workspace-message[data-role="agent"]')
        .filter({ hasText: `${SAYS.runBlock}${asked}` })
      const blocks = message.locator("[data-run-block]")
      if (!(await waitFor(async () => (await blocks.count()) === 2, 15_000))) {
        failures.push(`the reply drew ${await blocks.count()} Run blocks, not 2`)
        return
      }
      const [prints, waits] = [blocks.nth(0), blocks.nth(1)]
      // Run: the output shows under the block, then its exit and time.
      await prints.locator("[data-run-button]").click()
      const finished = await waitFor(
        async () => (await prints.getAttribute("data-run-state")) === "exited",
        20_000,
      )
      if (!finished) {
        failures.push("the first Run never ended")
        return
      }
      const grid = prints.locator('[data-slot="terminal-grid"]')
      const printed = await waitFor(
        async () => (await gridRows(grid)).some((row) => row.trim() === RUN_PRINTS),
        10_000,
      )
      if (!printed)
        failures.push(
          `Run's output does not show ${RUN_PRINTS}: ${JSON.stringify((await gridRows(grid)).filter((row) => row.trim()))}`,
        )
      // Scrolling the chat over the output scrolls the chat, not its history.
      await grid.hover()
      await run.page.mouse.wheel(0, -400)
      await run.page.waitForTimeout(300)
      if (await prints.locator('[data-slot="terminal-scrolled"]').count())
        failures.push("scrolling the chat over Run's output scrolled the output back")
      // As typed in a terminal: the prompt, then the command, above its output.
      // A long prompt wraps the command onto the next row, so the rows above
      // the output are read as one run of text.
      const rows = await gridRows(grid)
      const printedAt = rows.findIndex((row) => row.trim() === RUN_PRINTS)
      const above = rows.slice(0, Math.max(printedAt, 0)).join("").trimEnd()
      const prompted =
        above.endsWith(RUN_COMMAND) &&
        above.slice(0, -RUN_COMMAND.length).trim().length > 0
      if (printedAt < 0 || !prompted)
        failures.push(
          `Run's output does not show the command after a prompt above what it printed: ${JSON.stringify(rows.filter((row) => row.trim()))}`,
        )
      const status = prints.locator("[data-run-exit]")
      const words = (await status.count()) ? await status.innerText() : ""
      if (!/^exit 0 · \d/.test(words))
        failures.push(`the finished Run says ${JSON.stringify(words)}`)
      const runs = (await run.client.terminals.list()).filter(
        (each) => each.kind === "run" && each.conversationId === run.main.id,
      )
      if (!runs.some((each) => each.state === "exited" && each.exitStatus === 0))
        failures.push("the gateway does not list the finished run as exited 0")
      if (!(await prints.locator("[data-run-button]").innerText()).includes("Run again"))
        failures.push("a finished Run does not offer Run again")
      // The box stops at the last printed row, and the bar folds it away.
      await foldsItsOutput(
        prints,
        failures,
        run.evidence
          ? (state) =>
              message.screenshot({
                path: join(run.evidence, `run-${run.engine}-${step.index}-${state}.png`),
              })
          : undefined,
      )
      if (run.evidence)
        await message.screenshot({
          path: join(run.evidence, `run-${run.engine}-${step.index}.png`),
        })
      // The console fence runs its $ line only, and the stop icon interrupts it.
      await waits.locator("[data-run-button]").click()
      const running = await waitFor(
        async () => (await waits.getAttribute("data-run-state")) === "running",
        15_000,
      )
      if (!running) failures.push("the console fence's Run never started")
      else {
        await waits.getByRole("button", { name: "Stop" }).click()
        const stopped = await waitFor(
          async () => (await waits.getAttribute("data-run-state")) === "exited",
          10_000,
        )
        const exit = stopped
          ? await waits.locator("[data-run-exit]").getAttribute("data-run-exit")
          : ""
        if (!stopped) failures.push("Stop did not end the console fence's run")
        else if (exit === "0") failures.push("a stopped run says it exited 0")
      }
      // Run again starts a new run under the same block; the terminal icon opens it in a pane.
      await prints.locator("[data-run-button]").click()
      const again = await waitFor(async () => {
        const now = (await run.client.terminals.list()).filter(
          (each) => each.kind === "run" && each.conversationId === run.main.id,
        )
        return now.length === runs.length + 2
      }, 15_000)
      if (!again) failures.push("Run again did not start a new run")
      // Beside the chat: the chat stays in its own pane.
      const panesBefore = await page.locator(css.pane).count()
      await prints.getByRole("button", { name: "Open Beside" }).click()
      const pane = page.locator(".workspace-terminal-pane")
      if (
        !(await pane
          .first()
          .waitFor({ timeout: 5_000 })
          .then(
            () => true,
            () => false,
          ))
      )
        failures.push("the Run's terminal icon opened no pane")
      else {
        if ((await page.locator(css.pane).count()) !== panesBefore + 1)
          failures.push("the Run's terminal icon took the chat's pane, not one beside it")
        if (await prints.locator("[data-run-output]").count())
          failures.push("the Run still draws its output inline while a pane shows it")
        if (run.evidence)
          await page.screenshot({
            path: join(run.evidence, `run-${run.engine}-${step.index}-beside.png`),
          })
        const close = pane.first().getByRole("button", { name: /^(Close|End Terminal)$/ })
        await close.click()
      }
      // Leave no runs behind.
      for (const each of await run.client.terminals.list())
        if (each.kind === "run" && each.conversationId === run.main.id)
          await run.client.terminals.close(each.terminalId).catch(() => {})
      await showMainAgain(run, failures)
    })
  },
  async table(run, step, failures) {
    // The answer repeats the question, so it names this chat by its id: the
    // window draws that id as the chat's pill.
    const asked = prompt(run, STRESS.table, step)
    const text = `${asked} for ${run.main.id}`
    const message = run.page
      .locator('.workspace-message[data-role="agent"]')
      .filter({ hasText: `${SAYS.table}${asked}` })
    const tables = message.locator('[data-slot="markdown-table"]')
    const seen = await send(run, run.main.id, text, failures)
    // Words (the kit's ReplyMarkdown) parses each finished block once, so a
    // block that has finished is never drawn again while the rest streams.
    // The turn holds after its tables; while it holds, every cell of the
    // first table is marked, and each must be the same element once the
    // code, the diagram and the maths have arrived. (Only the fade-in spans
    // inside a cell go when the reply stops streaming.)
    const midStream = await waitFor(
      async () =>
        (await tables.count()) === 2 &&
        (await message.locator('[data-slot="mermaid-diagram"]').count()) === 0,
      TABLE_HOLD_MS + 15_000,
    )
    if (!midStream)
      failures.push("the window never drew the two tables before the rest of the answer")
    else
      await tables.first().evaluate((table) => {
        for (const cell of table.querySelectorAll("th, td")) cell.dataset.stressSeen = "1"
      })
    const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
    if (!turn) return
    // The gateway keeps the answer as it streamed: every line, in order, once.
    const said = (turn.parts ?? []).map((part) => part.text ?? "").join("")
    const expected = `${SAYS.table}${text}\n\n${[
      ...TABLE_LINES,
      "",
      ...CODE_LINES,
      "",
      ...MATH_LINES,
    ]
      .map((line) => `${line}\n`)
      .join("")}`
    if (said.trimEnd() !== expected.trimEnd())
      failures.push(`the table answer came back as ${JSON.stringify(said.slice(0, 120))}`)
    // The list previews the answer as prose: a table's pipes and dashes and
    // the code are left out (UI audit #3); the maths line is prose and stays.
    let listed
    const previewed = await waitFor(async () => {
      const { conversations } = await run.client.conversation.list({})
      listed = conversations.find((each) => each.conversationId === run.main.id)
      return (
        listed?.preview ===
        [`${SAYS.table}${text}`.trim(), ...MATH_LINES.filter(Boolean)].join(" ")
      )
    }, 10_000)
    if (!previewed)
      failures.push(
        `the list previews the table answer as ${JSON.stringify(listed?.preview)}`,
      )
    // The window draws both tables in the agent's message, and draws them
    // whole: no word split across lines, the narrow table inside the reply,
    // the wide one scrolling in its own panel with its hidden side faded.
    // Both tables whole: the last cell streamed is drawn, and the window has
    // finished revealing the answer (no word still in its fade-in span), so
    // every column has its final width.
    const lastCell = TABLE_LINES.at(-1).split("|").at(-2).trim()
    const drawn = await waitFor(
      async () =>
        (await tables.count()) === 2 &&
        (await tables.nth(1).getByText(lastCell, { exact: true }).count()) === 1 &&
        (await message.locator('span[class*="starting:"]').count()) === 0,
      15_000,
    )
    if (!drawn) {
      failures.push(
        `the window drew ${await tables.count()} of 2 tables, or not all of them`,
      )
      return
    }
    // The reply is drawn by Words, not react-markdown; the first table was
    // kept as it streamed; and the maths streamed last is drawn with KaTeX
    // from the TeX Words read, inline and as one display block, with no
    // dollar sign left in the reply's prose.
    const read = () =>
      message.evaluate(
        (element, id) => ({
          reply: element.querySelectorAll('[data-slot="reply-markdown"]').length,
          reactMarkdown: element.querySelectorAll('[data-slot="message-markdown"]')
            .length,
          redrawn:
            element
              .querySelector('[data-slot="markdown-table"]')
              ?.querySelectorAll(":is(th, td):not([data-stress-seen])").length ?? -1,
          inline: element.querySelectorAll(".katex:not(.katex-display .katex)").length,
          display: element.querySelectorAll(".katex-display").length,
          dollars: [...element.querySelectorAll('[data-slot="reply-markdown"] > p')]
            .map((paragraph) => paragraph.textContent)
            .filter((line) => line.includes("$")),
          pill: element.querySelectorAll(`[data-chat-pill="${id}"]`).length,
        }),
        run.main.id,
      )
    // KaTeX loads lazily, so the formulas may show as TeX for a moment.
    let words = await read()
    await waitFor(async () => {
      words = await read()
      return words.inline === 2 && words.display === 1
    }, 10_000)
    if (words.reply !== 1 || words.reactMarkdown !== 0)
      failures.push(
        `the answer is drawn by ${words.reply} ReplyMarkdown and ${words.reactMarkdown} MessageMarkdown, not Words alone`,
      )
    if (midStream && words.redrawn)
      failures.push(
        `${words.redrawn} cells of the first table were drawn again while the rest of the answer streamed`,
      )
    if (words.pill !== 1)
      failures.push(`the answer drew ${words.pill} pills for this chat's id, not 1`)
    if (words.inline !== 2 || words.display !== 1 || words.dollars.length)
      failures.push(
        `the answer's maths drew ${words.inline} inline and ${words.display} display formulas, with ${JSON.stringify(words.dollars)} left as text`,
      )
    if (run.evidence)
      await message.screenshot({
        path: join(run.evidence, `table-reply-${run.engine}-${step.index}.png`),
      })
    // Short runs each table holds, which a squeezed column used to split.
    const cut = [
      ["Price", "$16.99", "CAD", "$150"],
      ["Runtime", "Format", "Projectionist"],
    ]
    const measured = await message.evaluate((element, words) => {
      const lines = (root, word) => {
        const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
        for (let node = walker.nextNode(); node; node = walker.nextNode()) {
          const at = node.textContent.indexOf(word)
          if (at < 0) continue
          const range = document.createRange()
          range.setStart(node, at)
          range.setEnd(node, at + word.length)
          return new Set([...range.getClientRects()].map((rect) => Math.round(rect.top)))
            .size
        }
        return 0
      }
      const reply = element.getBoundingClientRect()
      return [...element.querySelectorAll('[data-slot="markdown-table"]')].map(
        (table, index) => {
          const port = table.querySelector('[data-slot="table-container"]')
          return {
            rows: table.querySelectorAll("tbody tr").length,
            missing: words[index].filter((word) => lines(table, word) === 0),
            split: words[index].filter((word) => lines(table, word) > 1),
            hidden: port.scrollWidth - port.clientWidth,
            within: table.getBoundingClientRect().right <= reply.right + 1,
            fadedRight: port.hasAttribute("data-hidden-right"),
          }
        },
      )
    }, cut)
    // Each table shows its data rows once, as streamed: a table's lines less
    // its heading and delimiter.
    const rows = TABLE_LINES.join("\n")
      .split("\n\n")
      .map((table) => table.split("\n").length - 2)
    for (const [index, table] of measured.entries()) {
      if (table.rows !== rows[index])
        failures.push(`table ${index + 1} shows ${table.rows} rows, not ${rows[index]}`)
      if (table.missing.length)
        failures.push(`table ${index + 1} does not show ${JSON.stringify(table.missing)}`)
      if (table.split.length)
        failures.push(
          `table ${index + 1} split ${JSON.stringify(table.split)} across lines`,
        )
      if (!table.within) failures.push(`table ${index + 1} is wider than the reply`)
    }
    // The table reads as part of the reply, not as small print in a grid:
    // its cells a step below the reply's own text (never under 85% of it),
    // column names smaller still and set in capitals, the first column a step
    // stronger, hairlines between columns fainter than the rules between
    // rows, and the panel close under the block before it (the copy strip
    // takes the place of that block's margin, not a second gap on top).
    const look = await message.evaluate((element) => {
      const alpha = (color) => {
        const slash = color.match(/\/\s*([\d.]+)(%?)\s*\)/)
        if (slash) return Number(slash[1]) / (slash[2] ? 100 : 1)
        const rgba = color.match(/rgba\([^)]*,\s*([\d.]+)\s*\)/)
        return rgba ? Number(rgba[1]) : 1
      }
      const prose = element.querySelector('[data-slot="reply-markdown"] > p')
      const table = element.querySelector('[data-slot="markdown-table"]')
      const before = table.previousElementSibling
      const cells = table.querySelectorAll("tbody td")
      const first = getComputedStyle(cells[0])
      const second = getComputedStyle(cells[1])
      const head = getComputedStyle(table.querySelector("th"))
      return {
        prose: prose ? parseFloat(getComputedStyle(prose).fontSize) : null,
        cell: parseFloat(second.fontSize),
        head: parseFloat(head.fontSize),
        caps: head.textTransform,
        firstWeight: Number(first.fontWeight),
        cellWeight: Number(second.fontWeight),
        columnRule: alpha(second.borderInlineStartColor),
        rowRule: alpha(getComputedStyle(cells[0].parentElement).borderBottomColor),
        gap: before
          ? Math.round(
              table.querySelector('[data-slot="table-shell"]').getBoundingClientRect()
                .top - before.getBoundingClientRect().bottom,
            )
          : null,
      }
    })
    if (look.prose === null)
      failures.push("the table answer has no paragraph to size cells against")
    else if (look.cell < look.prose * 0.85)
      failures.push(
        `table cells are ${look.cell}px beside ${look.prose}px prose: small print, not a step below`,
      )
    if (!(look.head < look.cell) || look.caps !== "uppercase")
      failures.push(
        `column names are ${look.head}px ${look.caps}, not smaller capitals above ${look.cell}px cells`,
      )
    if (!(look.firstWeight > look.cellWeight))
      failures.push(
        `the first column (${look.firstWeight}) is not a step stronger than the rest (${look.cellWeight})`,
      )
    if (!(look.columnRule < look.rowRule))
      failures.push(
        `column hairlines (${look.columnRule}) are not fainter than row rules (${look.rowRule})`,
      )
    // It used to sit 44px under a paragraph; now the strip and what is left
    // of the paragraph's margin make 36px.
    if (look.gap === null || look.gap > 40)
      failures.push(`the table sits ${look.gap}px under the block before it, past 40px`)
    const [narrow, wide] = measured
    if (narrow.hidden > 1 || narrow.fadedRight)
      failures.push(`the narrow table scrolls (${narrow.hidden} px hidden)`)
    if (wide.hidden <= 1)
      failures.push(
        `the wide table does not scroll in its panel (${wide.hidden} px hidden)`,
      )
    const port = tables.nth(1).locator('[data-slot="table-container"]')
    const faded = (left, right) =>
      waitFor(
        async () =>
          ((await port.getAttribute("data-hidden-left")) !== null) === left &&
          ((await port.getAttribute("data-hidden-right")) !== null) === right,
        5_000,
      )
    if (!(await faded(false, true)))
      failures.push(
        "the wide table does not fade its right side, where columns are hidden",
      )
    // Each table's copy control sits above its panel's top-right corner, never
    // on a column name: the wide table cuts its last column at the panel
    // edge, so a control inside the panel covered that name (UI audit #25).
    await tables.nth(1).hover()
    const copies = await message.evaluate((element) =>
      [...element.querySelectorAll('[data-slot="markdown-table"]')].map((table) => {
        const copy = table
          .querySelector('[data-slot="copy-button"]')
          ?.getBoundingClientRect()
        const panel = table
          .querySelector('[data-slot="table-container"]')
          .getBoundingClientRect()
        return copy
          ? { above: copy.bottom <= panel.top + 0.5, right: copy.right - panel.right }
          : null
      }),
    )
    for (const [index, copy] of copies.entries()) {
      if (!copy) failures.push(`table ${index + 1} has no copy control`)
      else if (!copy.above || Math.abs(copy.right) > 1)
        failures.push(
          `table ${index + 1}'s copy control is not above its top-right corner (${JSON.stringify(copy)})`,
        )
    }
    // Scrolled up, the way back to the latest sits on the composer's top
    // edge, half over it, rather than on a line of the reply (UI audit #28).
    const transcript = run.page.locator(css.focusedPane).locator(".workspace-transcript")
    // The way back shows once the reader is more than half a screen from the
    // end (`awayScreens` in transcript.tsx), so only a transcript that can
    // scroll that far owes it.
    if (
      await transcript.evaluate(
        (each) => each.scrollHeight - each.clientHeight > each.clientHeight * 0.5 + 40,
      )
    ) {
      const box = await transcript.boundingBox()
      await run.page.mouse.move(box.x + box.width / 2, box.y + box.height / 2)
      await run.page.mouse.wheel(0, -600)
      const jump = run.page
        .locator(css.focusedPane)
        .locator(".workspace-jump:not([data-hidden])")
      if (!(await waitFor(async () => (await jump.count()) === 1, 5_000)))
        failures.push("scrolled up, the way back to the latest did not show")
      else {
        const placed = await jump.evaluate((button) => {
          const at = button.getBoundingClientRect()
          const composer = button
            .closest(".workspace-pane")
            ?.querySelector(".workspace-dock .desktop-composer")
            ?.getBoundingClientRect()
          const middle = at.top + at.height / 2
          const hit = document.elementFromPoint(at.left + at.width / 2, middle)
          return {
            off: composer ? Math.round(middle - composer.top) : null,
            onTop: Boolean(hit?.closest(".workspace-jump")),
          }
        })
        if (placed.off === null || Math.abs(placed.off) > 6 || !placed.onTop)
          failures.push(
            `the way back to the latest is not on the composer's top edge: ${JSON.stringify(placed)}`,
          )
        await jump.click()
        await waitFor(async () => (await jump.count()) === 0, 5_000)
      }
    }
    if (run.evidence)
      await run.page.screenshot({
        path: join(run.evidence, `table-${run.engine ?? "window"}.png`),
      })
    // Scrolled to its end, the fade moves to the side now hidden.
    await port.evaluate((each) => each.scrollTo({ left: each.scrollWidth }))
    if (!(await faded(true, false)))
      failures.push("scrolled to its end, the wide table did not fade its left side")
    await codeBlocksShowTheirCode(message, failures)
    await codeStaysDark(run.page, message, failures)
  },

  async swipe(run, step, failures) {
    await run.exclusive(async () => {
      const page = run.page
      // An answer with a table wider than the reply: its panel scrolls
      // sideways, and a swipe over it scrolls it.
      const text = prompt(run, STRESS.table, step)
      const seen = await send(run, run.main.id, text, failures)
      const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
      if (!turn) return
      const message = page
        .locator('.workspace-message[data-role="agent"]')
        .filter({ hasText: `${SAYS.table}${text}` })
      const port = message
        .locator('[data-slot="markdown-table"]')
        .nth(1)
        .locator('[data-slot="table-container"]')
      if (!(await waitFor(async () => (await port.count()) === 1, 15_000))) {
        failures.push("the wide table to swipe over was not drawn")
        return
      }
      // The wide code block must have drawn its line before a swipe over it
      // can say anything about who took the swipe.
      await codeBlocksShowTheirCode(message, failures)
      if (failures.length) return
      await setSideRail(page, true, failures)
      if (failures.length) return
      try {
        const code = message.locator('[data-slot="code-block"]').nth(1)
        await sidewaysSwipes(run, step, message, port, code, failures)
      } finally {
        // Back as the other steps expect it, whatever failed: the rail off.
        await setSideRail(page, false, failures)
      }
    })
  },

  async long(run, step, failures) {
    const conversationId = randomUUID()
    await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    run.checked.push(conversationId)
    // Each answer streams LONG_CHUNKS chunks, so the history grows by about
    // 12 KB a turn and every save comes after a long one.
    const took = []
    let text
    for (let turn = 0; turn < step.turns; turn += 1) {
      text = prompt(run, STRESS.long, step, ` t${turn}`)
      const started = Date.now()
      await run.client.conversation.submit(
        run.client.conversation.prepare(conversationId, text),
      )
      const before = failures.length
      const { turn: done } = await settled(
        run,
        conversationId,
        new Set(),
        text,
        "completed",
        failures,
      )
      took.push(Date.now() - started)
      if (!done || failures.length > before) return
    }
    log(
      `long conversation: ${step.turns} turns, first ${took[0]} ms, last ${took.at(-1)} ms`,
    )
    // The newest answer is whole: every chunk, in order, once.
    const view = await run.client.conversation.read(conversationId)
    const answer = view.messages.find((each) => each.userText === text)
    const said = (answer?.parts ?? []).map((part) => part.text ?? "").join("")
    const chunks = [
      ...said.matchAll(new RegExp(` (\\d+) x{${LONG_CHUNK_BYTES}}`, "g")),
    ].map((match) => Number(match[1]))
    if (
      !said.startsWith(`${SAYS.long}${text}`) ||
      chunks.length !== LONG_CHUNKS ||
      chunks.some((index, at) => index !== at)
    )
      failures.push(
        `the last long answer came back with ${chunks.length} of ${LONG_CHUNKS} chunks: ${JSON.stringify(said.slice(0, 80))}`,
      )
    // Its agent is closed, so the next opening replays the whole saved
    // history: that takes no longer for its length than the budget allows,
    // and the answer read back after it is still whole.
    const short = randomUUID()
    await run.client.conversation.create({
      conversationId: short,
      agent: run.stack.agent,
    })
    run.checked.push(short)
    const one = prompt(run, STRESS.long, step, " alone")
    await run.client.conversation.submit(run.client.conversation.prepare(short, one))
    await settled(run, short, new Set(), one, "completed", failures)
    const alone = await reopened(run, short)
    const again = await reopened(run, conversationId)
    log(`long conversation reopened in ${again.ms} ms; one turn in ${alone.ms} ms`)
    const growth = reopenGrowthMs(step.turns)
    if (again.ms - alone.ms > growth)
      failures.push(
        `reopening ${step.turns} turns took ${again.ms} ms, one turn ${alone.ms} ms: over ${growth} ms more`,
      )
    if (again.view.transcriptState !== "complete")
      failures.push(
        `the reopened long conversation was read ${again.reads} times and is still ${again.view.transcriptState}`,
      )
    else {
      const kept = again.view.messages.find((each) => each.userText === text)
      if ((kept?.parts ?? []).map((part) => part.text ?? "").join("") !== said)
        failures.push(
          "the last long answer changed when the conversation was opened again",
        )
    }
    // Last, an answer longer than one view's frame: the view starts partway
    // through it, and its start is read back as earlier too.
    // Its view no longer lists the person's message, so the turn is followed
    // by its id.
    const oversized = prompt(run, STRESS.oversized, step)
    const { executionId } = await run.client.conversation.submit(
      run.client.conversation.prepare(conversationId, oversized),
    )
    const answered = await waitFor(async () => {
      const shown = await run.client.conversation.read(conversationId)
      const turn = shown.messages.find((each) => each.executionId === executionId)
      return turn && ended(turn) ? turn : undefined
    }, 60_000)
    if (answered?.status !== "completed")
      failures.push(`the oversized answer ended ${answered?.status ?? "never"}`)
    // A person opens the long conversation and writes in it.
    await openStressConversation(
      run.page,
      conversationId,
      await titleOf(run, conversationId),
    )
    // Its view lists only the newest answers. Scrolled back, the window reads
    // the rest a page at a time until the first turn shows.
    await readBackToStart(run, STRESS.long, step, failures)
    // The oversized answer, joined from its pieces, is drawn whole.
    const drawn = await run.page
      .locator(".workspace-message")
      .filter({ hasText: SAYS.oversized })
      .first()
      .textContent()
      .catch(() => "")
    const pieces = [
      ...(drawn ?? "").matchAll(new RegExp(`(\\d+) x{${LONG_CHUNK_BYTES}}`, "g")),
    ].map((match) => Number(match[1]))
    if (pieces.length !== OVERSIZED_CHUNKS || pieces.some((index, at) => index !== at))
      failures.push(
        `the oversized answer is drawn with ${pieces.length} of ${OVERSIZED_CHUNKS} chunks in order`,
      )
    const last = prompt(run, STRESS.reply, step, " after the long answers")
    const seen = await send(run, conversationId, last, failures)
    await settled(run, conversationId, seen, last, "completed", failures)
    await expectShown(run.page, `${SAYS.echo}${last}`, failures)
    await openStressConversation(run.page, run.main.id, run.main.title)
  },
}

/**
 * Scrolls the open conversation back to its top until its first turn shows:
 * the window reads the turns before those its view lists a page at a time as
 * the reader nears the top. Every turn of `step` (`phrase` … ` t<n>`) is then
 * drawn once, in order. A page that arrives keeps the reader on what they
 * were reading, and nothing says content was left out.
 */
async function readBackToStart(run, phrase, step, failures) {
  // The pane showing this conversation: an earlier step can leave other
  // panes open, a hidden one among them.
  const scroller = run.page
    .locator(`${css.transcript}:visible`)
    .filter({ hasText: phrase })
    .first()
  const turns = () =>
    scroller.evaluate((element, phrase) => {
      const said = [...element.querySelectorAll('.workspace-message[data-role="user"]')]
        .map((message) => message.textContent ?? "")
        .filter((text) => text.includes(phrase))
        .map((text) => Number(/ t(\d+) /.exec(`${text} `)?.[1] ?? -1))
      return {
        said,
        // A page may only lengthen the first turn drawn, adding no message.
        drawn: element.textContent?.length ?? 0,
        loading: Boolean(element.querySelector("[data-history-earlier]")),
        omitted: Boolean(element.querySelector('[data-history-extent="omitted"]')),
      }
    }, phrase)
  if (!(await waitFor(async () => (await turns()).loading, 10_000))) {
    const shown = await turns()
    failures.push(
      `a conversation longer than its view offers no earlier turns (shows ${JSON.stringify(shown.said)})`,
    )
    return
  }
  // At the top, the first message drawn: where its end is on screen before
  // the next page arrives, and after. A page joins whole turns above it, or
  // the start of its own turn inside it; either way, its end stays put.
  const top = () =>
    scroller.evaluate((element) => {
      element.scrollTop = 0
      const first = element.querySelector(".workspace-message")
      first?.setAttribute("data-stress-anchor", "")
      return first?.getBoundingClientRect().bottom ?? null
    })
  const anchored = () =>
    scroller.evaluate(
      (element) =>
        element.querySelector("[data-stress-anchor]")?.getBoundingClientRect().bottom ??
        null,
    )
  const end = Date.now() + 60_000
  let pages = 0
  let shown = await turns()
  while (shown.loading && Date.now() < end) {
    const before = await top()
    const count = shown.drawn
    if (!(await waitFor(async () => (await turns()).drawn > count, 15_000))) {
      failures.push(`scrolled to the top, nothing earlier arrived after ${pages} pages`)
      return
    }
    pages += 1
    const after = await anchored()
    if (before !== null && after !== null && Math.abs(after - before) > 2)
      failures.push(
        `a page of earlier turns moved what was being read by ${Math.round(after - before)}px`,
      )
    await scroller.evaluate((element) =>
      element
        .querySelector("[data-stress-anchor]")
        ?.removeAttribute("data-stress-anchor"),
    )
    shown = await turns()
  }
  const expected = Array.from({ length: step.turns }, (_, turn) => turn)
  if (shown.loading || JSON.stringify(shown.said) !== JSON.stringify(expected))
    failures.push(
      `read back over ${pages} pages, the window shows turns ${JSON.stringify(shown.said)}${shown.loading ? ", still loading" : ""}`,
    )
  if (shown.omitted) failures.push("the long conversation says some content is not shown")
  log(`long conversation read back to its first turn in ${pages} pages`)
  // Away from the end, the way back floats over words: solid at rest and
  // under the pointer, so nothing shows through it.
  const jump = scroller
    .locator("xpath=..")
    .getByRole("button", { name: "Jump to Latest" })
  const faces = []
  faces.push(await jump.evaluate((button) => getComputedStyle(button).backgroundColor))
  await jump.hover()
  // Its colour eases in, so wait for the hovered face rather than read mid-way.
  await waitFor(
    async () =>
      (await jump.evaluate((button) => getComputedStyle(button).backgroundColor)) !==
      faces[0],
    2_000,
  )
  faces.push(await jump.evaluate((button) => getComputedStyle(button).backgroundColor))
  await run.page.mouse.move(0, 0)
  for (const [state, face] of [
    ["at rest", faces[0]],
    ["hovered", faces[1]],
  ])
    if (!opaque(face))
      failures.push(`Jump to Latest shows the words through it ${state} (${face})`)
  // Back at the end, where the person writes next.
  await scroller.evaluate((element) => {
    element.scrollTop = element.scrollHeight
  })
}

/**
 * Whether a computed colour has no transparency: `rgba(…, a)` or a modern
 * `fn(… / a)` below 1, or `transparent`, lets what is behind show through.
 */
function opaque(color) {
  if (color === "transparent") return false
  const alpha =
    /\/\s*([\d.]+%?)\s*\)$/.exec(color)?.[1] ??
    /^rgba\([^)]*,\s*([\d.]+)\s*\)$/.exec(color)?.[1]
  if (alpha === undefined) return true
  return alpha.endsWith("%") ? Number.parseFloat(alpha) >= 100 : Number(alpha) >= 1
}

/** A menu row's height in nessa_ui's menu recipe, at the default scale. */
const MENU_ROW = 30

/**
 * The model menu's rows lead with their provider's tile as a square: the
 * row's icon slot is narrower than the tile, which once squeezed it into a
 * tall sliver. Resolves whether the menu closed again.
 */
async function modelTilesSquare(run, row, failures) {
  const page = run.page
  await row.locator(".desktop-chip-model").click({ timeout: 10_000 })
  await page.locator(css.modelPicker).waitFor({ timeout: 5_000 })
  // Every model, not only the provider the picker opens on.
  await page.locator(`${css.modelRail}[data-filter="all"]`).click()
  const tiles = page.locator(`${css.modelOption} [data-slot="model-level-picker-mark"]`)
  await tiles.first().waitFor({ timeout: 5_000 })
  const boxes = await tiles.evaluateAll((all) =>
    all.map((tile) => {
      const box = tile.getBoundingClientRect()
      const row = tile.closest('[role="option"]')?.getBoundingClientRect()
      return {
        width: Math.round(box.width * 10) / 10,
        height: Math.round(box.height * 10) / 10,
        fits:
          !!row &&
          box.top >= row.top - 0.5 &&
          box.bottom <= row.bottom + 0.5 &&
          box.left >= row.left - 0.5 &&
          box.right <= row.right + 0.5,
      }
    }),
  )
  for (const box of boxes)
    if (Math.abs(box.width - box.height) > 0.5 || box.width < 12)
      failures.push(
        `a model row's provider tile is ${box.width}x${box.height}px, not a square of 12px or more`,
      )
    else if (!box.fits) failures.push("a model row's provider tile spills out of its row")
  if (run.evidence)
    await page
      .locator(css.modelPicker)
      .screenshot({ path: join(run.evidence, `composer-${run.engine}-models.png`) })
      .catch(() => {})
  await page.keyboard.press(keys.escape)
  const closed = await page
    .locator(css.modelPicker)
    .waitFor({ state: "detached", timeout: 5_000 })
    .then(() => true)
    .catch(() => false)
  if (!closed) failures.push("Escape did not close the model picker")
  return closed
}

/**
 * Three panes stacked in a short window: a pane you are not in with nothing
 * typed shows its composer as a circle alone, one left with a draft keeps
 * its line, and pressing a circle focuses its pane with the caret in the
 * field. The panes it opens are closed again and the window size restored.
 */
async function stackedComposersFold(run, failures) {
  const page = run.page
  const size = page.viewportSize()
  const before = await page
    .locator(css.pane)
    .evaluateAll((panes) => panes.map((pane) => pane.dataset.paneKey))
  const home = await page.locator(css.focusedPane).getAttribute("data-pane-key")
  const composers = () =>
    page.locator(css.pane).evaluateAll((panes) =>
      panes.map((pane) => {
        const form = pane.querySelector(".desktop-composer")
        const wake = form?.querySelector(".desktop-composer-wake")
        const box = form?.getBoundingClientRect()
        return {
          key: pane.dataset.paneKey,
          focused: pane.hasAttribute("data-pane-focused"),
          running: form?.hasAttribute("data-running") ?? false,
          empty: form?.hasAttribute("data-empty") ?? false,
          width: Math.round(box?.width ?? 0),
          height: Math.round(box?.height ?? 0),
          circle: wake ? getComputedStyle(wake).display !== "none" : false,
          typing: !!form && document.activeElement === form.querySelector("textarea"),
        }
      }),
    )
  await page.setViewportSize({ width: 1200, height: 760 })
  try {
    await page.locator(css.focusedPane).locator("textarea").focus()
    for (let i = 0; i < 2; i += 1) {
      const count = await page.locator(css.pane).count()
      await page.keyboard.press(keys.splitDown)
      if (
        !(await waitFor(
          async () => (await page.locator(css.pane).count()) > count,
          5_000,
        ))
      ) {
        failures.push(`⇧⌘\\ left ${count} panes, not ${count + 1}`)
        return
      }
    }
    // The new panes are empty sessions; the first one opened is now a pane you are not in.
    const folded = async () =>
      (await composers()).filter((c) => !c.focused && c.empty && !c.running)
    const circles = await waitFor(async () => {
      const quiet = await folded()
      return (
        quiet.length > 0 &&
        quiet.every((c) => c.circle && c.width <= 40 && c.height <= 40)
      )
    }, 5_000)
    const quiet = await folded()
    if (quiet.length === 0) failures.push("no empty pane you are not in to fold")
    else if (!circles)
      failures.push(
        `an empty pane you are not in did not fold to a circle: ${JSON.stringify(quiet)}`,
      )
    if ((await composers()).some((c) => c.focused && c.circle))
      failures.push("the pane you are in shows its composer as a circle")
    if (run.evidence)
      await page.screenshot({
        path: join(run.evidence, `composer-${run.engine}-stacked.png`),
      })
    // A draft keeps its line when its pane is left.
    const drafted = (await composers()).find((c) => c.focused)
    const other = quiet[0]
    if (drafted && other) {
      await page
        .locator(`[data-pane-key="${drafted.key}"] textarea`)
        .fill("kept as a line")
      await page
        .locator(`[data-pane-key="${other.key}"] .desktop-composer-wake`)
        .click({ timeout: 5_000 })
      const pressed = await waitFor(async () => {
        const now = (await composers()).find((c) => c.key === other.key)
        return now?.focused && now.typing && !now.circle
      }, 5_000)
      if (!pressed)
        failures.push(
          `pressing pane ${other.key}'s circle left it ${JSON.stringify((await composers()).find((c) => c.key === other.key))}`,
        )
      const kept = (await composers()).find((c) => c.key === drafted.key)
      if (!kept || kept.circle || kept.width <= 40)
        failures.push(`a pane left with a draft folded: ${JSON.stringify(kept)}`)
      await page.locator(`[data-pane-key="${drafted.key}"] textarea`).fill("")
    }
  } finally {
    // Close what this opened, newest first, and give the window back its size.
    const opened = (
      await page
        .locator(css.pane)
        .evaluateAll((panes) => panes.map((pane) => pane.dataset.paneKey))
    ).filter((key) => !before.includes(key))
    for (const key of opened.reverse()) {
      await page.locator(`[data-pane-key="${key}"] ${css.paneHeader}`).click()
      await page.keyboard.press(keys.closePane)
      if (
        !(await waitFor(
          async () => (await page.locator(`[data-pane-key="${key}"]`).count()) === 0,
          5_000,
        ))
      )
        failures.push(`⌘W did not close pane ${key} the step opened`)
    }
    if (size) await page.setViewportSize(size)
    // Back in the pane the step started in, as later steps expect.
    if (
      home &&
      (await page.locator(css.focusedPane).getAttribute("data-pane-key")) !== home
    ) {
      await page.locator(`[data-pane-key="${home}"] ${css.paneHeader}`).click()
      const back = await waitFor(
        async () =>
          (await page.locator(css.focusedPane).getAttribute("data-pane-key")) === home,
        5_000,
      )
      if (!back) failures.push(`pane ${home} was not focused again after the step`)
    }
  }
}

/**
 * The open menu's rows as drawn: whether each leads with its icon, its
 * height, whether it is checked, and where its check and shortcut sit.
 */
function menuRows(page) {
  return page
    .locator(css.menu)
    .first()
    .evaluate((menu) =>
      [...menu.querySelectorAll('[role^="menuitem"]')].map((row) => {
        const shortcut = row.querySelector('[data-slot$="menu-shortcut"]')
        const check = row.querySelector('[data-slot$="menu-item-indicator"] svg')
        const box = (element) => element?.getBoundingClientRect()
        return {
          name: row.textContent?.trim() ?? "",
          leads: row.firstElementChild?.tagName.toLowerCase() === "svg",
          height: Math.round(box(row).height),
          radio: row.getAttribute("role") === "menuitemradio",
          checked: row.getAttribute("aria-checked") === "true",
          shortcut: shortcut?.textContent?.trim() ?? null,
          shortcutLeft: shortcut ? Math.round(box(shortcut).left) : null,
          shortcutRight: shortcut ? Math.round(box(shortcut).right) : null,
          checkRight: check ? Math.round(box(check).right) : null,
        }
      }),
    )
}

/**
 * Two panes side by side are regions of one flat chat surface, not cards
 * (`flatSurfaceFailures`): one hairline between them, mid-gutter.
 */
/**
 * The window shows a thread's report as one line — the thread's chat as a
 * pill with its title, and what it did — never the report's text or ids;
 * pressed, the pill brings that chat to the front, in the focused pane.
 */
async function reportLine(run, report, failures) {
  const thread = /^\[Thread report\] Thread (\S+) /.exec(report.userText)?.[1]
  if (!thread) {
    failures.push(`the report did not name its thread: ${report.userText.slice(0, 120)}`)
    return
  }
  const watchedTurn = /\(execution ([^)]+)\):/.exec(report.userText)?.[1]
  if (
    !watchedTurn ||
    report.agent?.conversationId !== thread ||
    report.agent.executionId !== watchedTurn
  )
    failures.push("the thread report did not retain its child agent as the author")
  const line = run.page.locator(css.threadReport).filter({
    has: run.page.locator(`[data-chat-pill="${thread}"]`),
  })
  const shown = await line
    .first()
    .waitFor({ timeout: 15_000 })
    .then(() => true)
    .catch(() => false)
  if (!shown) {
    failures.push("the thread's report is not one line with the thread's pill")
    return
  }
  const title = await titleOf(run, thread)
  const said = (await line.first().innerText()).replace(/\s+/g, " ").trim()
  if (
    !said.startsWith("Agent report from ") ||
    !said.includes(title) ||
    !said.endsWith("responded")
  )
    failures.push(`the report line does not name its agent: "${said}"`)
  if (run.evidence)
    await line.first().screenshot({
      path: join(run.evidence, `thread-report-${run.engine}.png`),
    })
  if (
    /finished the turn you sent it|execution /.test(
      await run.page.locator(css.pane).first().innerText(),
    )
  )
    failures.push("the window shows the report's text, not just its line")
  await line.first().locator(`[data-chat-pill="${thread}"]`).click()
  // In front: the focused pane shows it, which its list row says.
  const front = await waitFor(
    async () =>
      (await selectedStressConversation(run.page, await titleOf(run, thread))) === thread,
    10_000,
  )
  if (!front)
    failures.push("pressing the report's pill did not bring its chat to the front")
  await openStressConversation(run.page, run.main.id, run.main.title)
}

/**
 * `oneHeaderPerChat` once the panes come to rest after a column folds or
 * opens: what it measures is read again until it holds, or the columns'
 * motion has had its time; what still fails then is the step's.
 */
async function headersLand(page, when, failures) {
  let last = []
  await waitFor(async () => {
    last = []
    await oneHeaderPerChat(page, when, last)
    return last.length === 0
  }, 5_000)
  failures.push(...last)
}

/**
 * How many tabs or tab lists the window draws for its chats: any shown one
 * outside the right panel and outside what a chat shows (a tool card's own
 * Input/Output tabs are the chat's content, not tabs for chats); one in a
 * pane's header counts.
 */
function chatTabs(page) {
  return page.evaluate(
    ({ pane, paneHeader }) =>
      [...document.querySelectorAll('[role="tablist"], [role="tab"]')].filter(
        (tab) =>
          tab.getClientRects().length > 0 &&
          !tab.closest("[inert], #workspace-panel") &&
          (!tab.closest(pane) || tab.closest(paneHeader)),
      ).length,
    { pane: css.pane, paneHeader: css.paneHeader },
  )
}

/**
 * The chats have no tabs (C1): no tab strip, and each top-row pane has one header —
 * its name, menu and close — in the titlebar's row, none of it on the
 * window's controls (the cluster on the left, the panel's own on the
 * right), and its picture up to the window's top edge, behind them.
 */
async function oneHeaderPerChat(page, when, failures) {
  const state = await page.evaluate(
    ({ titlebar, pane, header }) => {
      const box = (e) => {
        const b = e.getBoundingClientRect()
        return { left: b.left, top: b.top, right: b.right, bottom: b.bottom }
      }
      const shown = (e) => e.getClientRects().length > 0
      return {
        bar: document.querySelector(titlebar).getBoundingClientRect().height,
        // The titlebar is unified with the window: no fill and no rule of its
        // own, so what is under it (the sidebar, a pane's picture) runs up.
        barPaint: (() => {
          const style = getComputedStyle(document.querySelector(titlebar))
          return { fill: style.backgroundColor, rule: style.boxShadow }
        })(),
        // Text the titlebar draws itself: a chat's name there is a tab again.
        barText: document.querySelector(titlebar).innerText.trim(),
        names: [...document.querySelectorAll(`${pane} .workspace-pane-name`)]
          .map((name) => name.textContent.trim())
          .filter(Boolean),
        controls: [
          ...document.querySelectorAll(
            `${titlebar} .workspace-titlebar-navigation button, ${titlebar} .workspace-titlebar-trailing button`,
          ),
        ]
          .filter(shown)
          .map(box),
        panes: [...document.querySelectorAll(pane)].map((each) => ({
          headers: [...each.querySelectorAll(header)].map((h) => ({
            box: box(h),
            parts: [...h.querySelectorAll("button, .workspace-pane-name")]
              .filter(shown)
              .map(box),
          })),
          box: box(each),
          scroll: {
            top: each.scrollTop,
            left: each.scrollLeft,
            height: each.scrollHeight,
            viewport: each.clientHeight,
            overflow: getComputedStyle(each).overflow,
          },
          picture: [...each.querySelectorAll(".desktop-header, .desktop-header-image")]
            .filter(shown)
            .map(box)[0],
        })),
      }
    },
    { titlebar: css.titlebar, pane: css.pane, header: css.paneHeader },
  )
  const overlaps = (a, b) =>
    a.left < b.right - 1 &&
    b.left < a.right - 1 &&
    a.top < b.bottom - 1 &&
    b.top < a.bottom - 1
  if (await chatTabs(page)) failures.push(`${when}: a tab strip is drawn for the chats`)
  if (state.barPaint.fill !== "rgba(0, 0, 0, 0)" || state.barPaint.rule !== "none")
    failures.push(
      `${when}: the titlebar paints a toolbar: ${JSON.stringify(state.barPaint)}`,
    )
  for (const name of state.names)
    if (state.barText.includes(name))
      failures.push(`${when}: the titlebar names the chat "${name}"`)
  for (const [at, pane] of state.panes.entries()) {
    const name = `${when}, pane ${at + 1}`
    if (pane.scroll.top !== 0 || pane.scroll.left !== 0)
      failures.push(`${name}: its root scrolled: ${JSON.stringify(pane.scroll)}`)
    if (pane.headers.length !== 1) {
      failures.push(`${name}: ${pane.headers.length} headers, not one`)
      continue
    }
    const [header] = pane.headers
    if (header.box.top < -1 || header.box.bottom > state.bar + 1)
      failures.push(
        `${name}: its header is not in the titlebar row: ${JSON.stringify({ header: header.box, pane: pane.box, scroll: pane.scroll })}`,
      )
    for (const part of header.parts)
      for (const control of state.controls)
        if (overlaps(part, control))
          failures.push(
            `${name}: its header sits on a window control: ${JSON.stringify({ part, control })}`,
          )
    if (
      pane.picture &&
      (pane.picture.top > 0.5 || pane.picture.left > pane.box.left + 0.5)
    )
      failures.push(
        `${name}: its picture does not reach the window's top and its pane's left: ${JSON.stringify({ picture: pane.picture, pane: pane.box })}`,
      )
  }
  return state
}

async function flatPanes(page, failures) {
  const now = async () => ({
    panes: await page.locator(css.pane).count(),
    hairlines: await page.locator(`${css.chat} ${css.paneEdge}[data-axis="x"]`).count(),
    broken: await flatSurfaceFailures(page),
    // Room between each pane's composer and the pane's right edge: one
    // gutter in every pane, a narrow one too (UI audit #27).
    gutters: await page.locator(css.pane).evaluateAll((panes) =>
      panes.map((pane) => {
        const composer = pane.querySelector(".workspace-dock .desktop-composer")
        return composer
          ? Math.round(
              pane.getBoundingClientRect().right - composer.getBoundingClientRect().right,
            )
          : null
      }),
    ),
  })
  const holds = (state) =>
    state.panes === 2 &&
    state.hairlines === 1 &&
    state.broken.length === 0 &&
    state.gutters.every((gutter) => gutter !== null && gutter >= 16)
  if (await waitFor(async () => holds(await now()), 10_000)) return
  failures.push(`the panes are not one flat surface: ${JSON.stringify(await now())}`)
}

/**
 * Opens a new session beside the one pane (⇧⌘N), checks its header reads
 * "New session" and is shown while the conversation's stays as it was, then
 * closes it again (⌘W).
 */
/**
 * The titlebar has no Back or Forward: the window keeps no history for them
 * to walk, and two buttons that never did anything were noise (UI audit #31).
 */
async function noHistoryButtons(page, failures, where) {
  const titlebar = page.locator(".workspace-titlebar, .settings-titlebar")
  if (!(await titlebar.count())) {
    failures.push(`${where} has no titlebar to look at`)
    return
  }
  const named = await titlebar.evaluateAll((bars) =>
    bars.flatMap((bar) =>
      [...bar.querySelectorAll("button")]
        .map((button) => button.getAttribute("aria-label") ?? "")
        .filter((label) => /^Go (Back|Forward)\b/.test(label)),
    ),
  )
  if (named.length) failures.push(`${where}'s titlebar shows ${JSON.stringify(named)}`)
}

async function homeBesideNamed(page, failures) {
  await page.keyboard.press(keys.newSessionBeside)
  if (
    !(await waitFor(async () => (await page.locator(css.pane).count()) === 2, 10_000))
  ) {
    failures.push(
      `a new session beside left ${await page.locator(css.pane).count()} panes, not 2`,
    )
    return
  }
  const home = page.locator(css.focusedPane)
  const named = await waitFor(
    async () =>
      (await home.locator(css.paneHome).count()) === 1 &&
      (await home
        .locator("[data-shown] .workspace-pane-title")
        .textContent()
        .catch(() => "")) === "New session",
    5_000,
  )
  if (!named)
    failures.push(
      `the new session beside is not named: ${JSON.stringify(
        await home.locator(".workspace-pane-title").allTextContents(),
      )}`,
    )
  // A third pane changes the arrangement while this home's scene is already
  // drawn: it stays drawn on every frame of the change.
  const scene = page.locator(`${css.paneHome} .desktop-night-scene[data-scene-complete]`)
  if (!(await waitFor(async () => (await scene.count()) === 1, 5_000))) {
    failures.push("the new session beside never finished drawing its scene")
    return
  }
  const split = await lookHolds(page, "a third pane beside a drawn scene")
  await page.keyboard.press(keys.newSessionBeside)
  if (
    !(await waitFor(async () => (await page.locator(css.pane).count()) === 3, 10_000))
  ) {
    failures.push(
      `another session beside left ${await page.locator(css.pane).count()} panes, not 3`,
    )
    return
  }
  await split(failures)
  // Close both homes: the focused new one, then the first, focused by a click.
  for (const left of [2, 1]) {
    const focused = page.locator(css.focusedPane)
    if ((await focused.locator(css.paneHome).count()) !== 1)
      await page
        .locator(css.paneHome)
        .first()
        .click({ position: { x: 20, y: 120 } })
    await page.keyboard.press(keys.closePane)
    if (
      !(await waitFor(
        async () => (await page.locator(css.pane).count()) === left,
        10_000,
      ))
    ) {
      failures.push(`⌘W left ${await page.locator(css.pane).count()} panes, not ${left}`)
      return
    }
  }
}

/**
 * Asks `conversationId`'s agent what the window shows, until what it says
 * meets `holds`, asked in the focused pane's composer. The window tells the
 * gateway what it shows on its own request, so the turn can start a moment
 * before the gateway has the change. Resolves with the answer that met
 * `holds`, else `null`; the window shows each answer.
 */
/**
 * The main conversation's agent calls a window tool (`phrase` names which)
 * with `args`, asked in its own pane's composer (on the client only when
 * `covered`, as nothing in the window can be typed in); what the tool
 * answered, its error included, or null.
 */
async function windowControl(run, phrase, args, step, extra, failures, options = {}) {
  const text = prompt(run, phrase, step, `${extra} args:${JSON.stringify(args)} :args`)
  // Typed into the main conversation's own pane, which a change may have
  // left unfocused: the pane the last answer said was its own (`you.pane`,
  // counted in reading order, as the panes are laid out), else the focused one.
  const panes = run.page.locator(css.pane)
  const own = run.controlPane
  const where = (
    own && own <= (await panes.count())
      ? panes.nth(own - 1)
      : run.page.locator(css.focusedPane)
  ).locator(css.conversationDock)
  let seen = new Set()
  if (options.covered)
    await run.client.conversation.submit(
      run.client.conversation.prepare(run.main.id, text),
    )
  else seen = await send(run, run.main.id, text, failures, undefined, where)
  const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
  if (!turn) return null
  const reply = (turn.parts ?? []).map((part) => part.text ?? "").join("")
  const at = reply.indexOf(SAYS.control)
  try {
    const said = JSON.parse(reply.slice(at + SAYS.control.length))
    run.controlPane = said?.window?.you?.shown ? said.window.you.pane : undefined
    return said
  } catch {
    failures.push(
      `the window tool turn did not say what came of it: ${reply.slice(0, 120)}`,
    )
    return null
  }
}

async function windowRead(run, conversationId, step, extra, failures, holds) {
  let said
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    const text = prompt(run, STRESS.window, step, `${extra} a${attempt}`)
    // The pane asking is the focused one: its composer is the one typed in.
    const seen = await send(
      run,
      conversationId,
      text,
      failures,
      undefined,
      run.page.locator(css.focusedPane).locator(css.conversationDock),
    )
    const { turn } = await settled(run, conversationId, seen, text, "completed", failures)
    if (!turn) return null
    const reply = (turn.parts ?? []).map((part) => part.text ?? "").join("")
    const at = reply.indexOf(SAYS.window)
    try {
      said = JSON.parse(reply.slice(at + SAYS.window.length))
    } catch {
      failures.push(`the window turn did not say what it read: ${reply.slice(0, 120)}`)
      return null
    }
    await expectShown(run.page, `${SAYS.window}{`, failures)
    if (holds(said)) return said
    await sleep(500)
  }
  failures.push(`${extra.trim()}, the agent read the window as ${JSON.stringify(said)}`)
  return null
}

/**
 * Close `conversationId`'s agent, then open it again with a read, timed;
 * the view is read on to the head, as one read takes a bounded number of
 * records.
 */
async function reopened(run, conversationId) {
  await run.client.conversation.close(conversationId)
  const started = Date.now()
  let view = await run.client.conversation.read(conversationId)
  const ms = Date.now() - started
  let reads = 1
  for (; view.transcriptState !== "complete" && reads < 20; reads += 1) {
    await sleep(100)
    view = await run.client.conversation.read(conversationId)
  }
  return { ms, view, reads }
}

/**
 * Opens Settings › Connections › MCP servers, as a person does, and waits
 * for its list. Connections opens on MCP servers, which needs no vault: a
 * passphrase prompt there (a locked vault) is a failure the caller reports.
 */
async function openMcpServers(page) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await page.locator(css.settingsCategory, { hasText: names.connections }).click()
  await page.waitForSelector(selectorFor.mcpServersIn("listed"), { timeout: 10_000 })
  const prompted = await page.locator('[data-vault-dialog="unlock"]').isVisible()
  const tab = page.locator(css.settingsTab, { hasText: names.integrations })
  if ((await tab.getAttribute("aria-selected")) !== "true") await tab.click()
  return { prompted }
}

/** Chooses `item` from the ⋯ menu of the MCP server row named `name`. */
async function mcpMenu(page, name, item) {
  await page.getByRole("button", { name: names.mcp.more(name), exact: true }).click()
  await page
    .locator(selectorFor.mcpMenuOf(name))
    .getByRole("menuitem", { name: item, exact: true })
    .click()
}

/** Opens Settings › Connections › Linked devices, as a person does. */
async function openLinkedDevices(page) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await page.locator(css.settingsCategory, { hasText: names.connections }).click()
  // Connections opens on Accounts, whose rows read the vault: a locked Nessa
  // asks to be unlocked before the tabs take a click.
  await page
    .locator('[data-vault-dialog="unlock"]')
    .waitFor({ timeout: 2_000 })
    .then(() => unlockVault(page))
    .catch(() => {})
  await page.locator(css.settingsTab, { hasText: names.linkedDevices }).click()
}

/**
 * Asks for setup again where a person does: Settings › Advanced › Setup, its
 * Replay button. Resolves how many times the host was asked. Leaves Settings.
 */
async function replayOnboarding(page, evidence) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await page.locator(css.settingsCategory, { hasText: names.advanced }).click()
  await page.locator(css.settingsTab, { hasText: names.setupTab }).click()
  if (evidence)
    await page
      .screenshot({ path: join(evidence, "onboarding-settings.png") })
      .catch(() => {})
  const before = await page.evaluate(() => window.__fakeHostReplays)
  await page
    .locator(selectorFor.settingRow("replay-onboarding"))
    .getByRole("button", { name: names.replayOnboarding, exact: true })
    .click()
  await waitFor(
    async () => (await page.evaluate(() => window.__fakeHostReplays)) > before,
    5_000,
  )
  const asked = (await page.evaluate(() => window.__fakeHostReplays)) - before
  await closeSettings(page)
  return asked
}

/** The menu-bar panel as the host opens it, over the run's gateway. */
async function panelWindow(run) {
  const page = await run.page.context().newPage()
  await page.setViewportSize({ width: 420, height: 760 })
  await page.addInitScript(gatewayHost, {
    endpoint: run.stack.endpoint,
    credential: run.stack.credential,
    panel: true,
  })
  await page.goto(`${new URL(run.page.url()).origin}/index.html`)
  return page
}

/** Waits until the panel's strip leads with `title`, the selected tab's name. */
async function panelNamed(page, title, failures) {
  const named = await waitFor(
    async () =>
      (
        await page
          .locator(css.panelCurrent)
          .innerText()
          .catch(() => "")
      ).includes(title),
    15_000,
  )
  if (!named)
    failures.push(`the panel's strip does not lead with ${JSON.stringify(title)}`)
}

/**
 * A setup window as the host opens it, over the run's gateway, recording
 * each beat of its opening as it comes: the beat, and whether the window's
 * heading could be seen by then.
 */
async function setupWindow(run, prepare) {
  const page = await run.page.context().newPage()
  // Before the page loads, so nothing it asks on its first frames gets past.
  await run.stack.vault.install(page)
  await prepare?.(page)
  await page.addInitScript(gatewayHost, {
    endpoint: run.stack.endpoint,
    credential: run.stack.credential,
    setup: true,
  })
  await page.addInitScript(() => {
    window.__setupBeats = []
    window.__setupLeft = false
    new MutationObserver(() => {
      const shell = document.querySelector(".nessa-setup-window")
      if (shell?.hasAttribute("data-leaving")) window.__setupLeft = true
      const beat = shell?.dataset.beat
      if (!beat || window.__setupBeats.at(-1)?.beat === beat) return
      // Seen as a person sees it: a pane held hidden or clear until the
      // window opens is in the page from the first frame.
      const heading = document.getElementById("nessa-setup-heading")
      const headingSeen = Boolean(
        heading?.checkVisibility({ checkVisibilityCSS: true, checkOpacity: true }),
      )
      // Where the window is drawn as the beat starts, transform included.
      const { left, top, right, bottom } = shell.getBoundingClientRect()
      const drawn = { left, top, right, bottom, width: innerWidth, height: innerHeight }
      window.__setupBeats.push({ beat, headingSeen, drawn })
    }).observe(document, { subtree: true, childList: true, attributes: true })
    // The flight is the window's one Web Animation: the box it took off
    // from and the keyframes and timing it was given, taken as it starts.
    window.__setupFlights = []
    const animate = Element.prototype.animate
    Element.prototype.animate = function (...args) {
      const animation = animate.apply(this, args)
      if (this.classList.contains("nessa-setup-window")) {
        const { left, top, width, height } = this.getBoundingClientRect()
        window.__setupFlights.push({
          from: { left, top, width, height },
          frames: animation.effect.getKeyframes(),
          duration: animation.effect.getComputedTiming().duration,
        })
      }
      return animation
    }
  })
  await page.goto(`${new URL(run.page.url()).origin}/index.html?surface=setup`)
  await page.locator(".nessa-setup-window").waitFor()
  // Onboarding does not ask for the vault on its own (#1340), locked or not:
  // nothing in it needs protected data, so no unlock dialog is drawn.
  await page.waitForTimeout(1500)
  if ((await page.locator("[data-vault-dialog]").count()) !== 0) {
    await page.close()
    throw new Error("the setup window opened a vault dialog on its own")
  }
  return page
}

/**
 * The welcome's words are all there as the window opens: the name is plain
 * text, not letters typed in one at a time, and the line under it and its
 * way on do not wait for it.
 */
async function welcomedAtOnce(page, failures) {
  const read = () =>
    page.evaluate(() => {
      const heading = document.getElementById("nessa-setup-heading")
      const seen = (element) =>
        Boolean(
          element?.checkVisibility({ checkVisibilityCSS: true, checkOpacity: true }),
        )
      const begin = [...document.querySelectorAll(".nessa-setup-pane button")].find(
        (button) => button.textContent.trim() === "Get started",
      )
      return {
        text: heading?.textContent,
        letters: heading?.querySelectorAll("[data-slot=type-in-letter]").length ?? 0,
        painted: heading?.querySelectorAll("[data-slot=gradient-text]").length ?? 0,
        waiting: document.querySelectorAll(
          ".nessa-setup-pane [data-typed], .nessa-setup-after",
        ).length,
        begin: seen(begin),
      }
    })
  // The pane takes a moment to slide in; typing the name took seconds.
  await waitFor(async () => (await read()).begin, 1_000)
  const welcome = await read()
  if (welcome.text !== names.setup.welcome)
    failures.push(`the welcome reads ${JSON.stringify(welcome.text)} as it opens`)
  if (welcome.letters || welcome.waiting)
    failures.push(`the welcome is typed in: ${JSON.stringify(welcome)}`)
  if (!welcome.begin) failures.push("Get started is not there as the welcome opens")
  if (welcome.painted)
    failures.push("the welcome paints the name in colour; setup is graphite like the app")
}

/**
 * The window is dark only, and so are its code and diagrams: with the OS
 * turned to light, the answer's code block is still drawn on a dark ground and
 * its Mermaid diagram's boxes are still dark (UI audit #29). The code is read
 * from the screen, as the highlighter draws its own colours; the diagram from
 * its boxes' fills. The page's appearance is put back after, whatever the
 * check found.
 */
/**
 * The code each fenced block in a `table` answer holds, in order, read from
 * the lines the scripted agent streams (`CODE_LINES`): Mermaid is a diagram,
 * not a block.
 */
function fencedCode() {
  const fences = []
  let open = null
  for (const line of CODE_LINES) {
    if (line.startsWith("```")) {
      if (open === null) open = { language: line.slice(3), lines: [] }
      else {
        if (open.language !== "mermaid") fences.push(open.lines.join("\n"))
        open = null
      }
    } else if (open !== null) open.lines.push(line)
  }
  return fences
}

/**
 * Every code block in the answer shows its own code and takes room. The
 * highlighter draws its lines inside a shadow root, so a block whose engine
 * never drew stays an empty, zero-height `<pre>` that every check reading
 * the block's box or colour would pass.
 */
async function codeBlocksShowTheirCode(message, failures) {
  const want = fencedCode()
  const read = () =>
    message.evaluate((element) =>
      [...element.querySelectorAll('[data-slot="code-block"]')].map((block) => ({
        text: [...block.querySelectorAll("*")]
          .map((each) => each.shadowRoot?.textContent ?? "")
          .join(""),
        height: block.getBoundingClientRect().height,
      })),
    )
  const shows = (blocks) =>
    blocks.length === want.length &&
    blocks.every(
      (block, index) =>
        block.height > 0 &&
        want[index].split("\n").every((line) => block.text.includes(line)),
    )
  if (await waitFor(async () => shows(await read()), 20_000)) return
  const blocks = await read()
  if (blocks.length !== want.length)
    failures.push(`the answer has ${blocks.length} code blocks, not ${want.length}`)
  for (const [index, block] of blocks.entries()) {
    const missing = (want[index] ?? "")
      .split("\n")
      .filter((line) => !block.text.includes(line))
    if (missing.length > 0 || block.height <= 0)
      failures.push(
        `code block ${index + 1} does not show its code (${Math.round(block.height)} px high, missing ${JSON.stringify(missing)})`,
      )
  }
}

async function codeStaysDark(page, message, failures) {
  // The first block, with its one short line; the second is the wide one.
  const blocks = message.locator('[data-slot="code-block"]')
  const block = blocks.first()
  const diagram = message.locator('[data-slot="mermaid-diagram"] svg')
  const drawn = await waitFor(
    async () => (await blocks.count()) === 2 && (await diagram.count()) >= 1,
    20_000,
  )
  if (!drawn) {
    failures.push("the answer's code block or diagram was not drawn")
    return
  }
  await page.emulateMedia({ colorScheme: "light" })
  try {
    await block.scrollIntoViewIfNeeded()
    const codeLight = async () => {
      const box = await block.boundingBox()
      const shot = (await page.screenshot()).toString("base64")
      return page.evaluate(
        async ([png, box, scale]) => {
          const image = new Image()
          image.src = `data:image/png;base64,${png}`
          await image.decode()
          const canvas = document.createElement("canvas")
          canvas.width = image.width
          canvas.height = image.height
          const ctx = canvas.getContext("2d")
          ctx.drawImage(image, 0, 0)
          // The block's right end, past its one short line of code.
          const [r, g, b] = ctx.getImageData(
            Math.round((box.x + box.width - 24) * scale),
            Math.round((box.y + box.height / 2) * scale),
            1,
            1,
          ).data
          return Math.round((r + g + b) / 3)
        },
        [shot, box, await page.evaluate(() => window.devicePixelRatio)],
      )
    }
    // The lightest box in the diagram, 0 to 255.
    const diagramLight = () =>
      message.evaluate((element) => {
        const swatch = document.createElement("canvas").getContext("2d")
        let lightest = 0
        for (const shape of element.querySelectorAll(
          '[data-slot="mermaid-diagram"] svg .node :is(rect, polygon, circle, path)',
        )) {
          const fill = getComputedStyle(shape).fill
          if (!fill || fill === "none") continue
          swatch.clearRect(0, 0, 1, 1)
          swatch.fillStyle = fill
          swatch.fillRect(0, 0, 1, 1)
          const [r, g, b, a] = swatch.getImageData(0, 0, 1, 1).data
          if (a > 128) lightest = Math.max(lightest, Math.round((r + g + b) / 3))
        }
        return lightest
      })
    // Anything following the OS redraws light within a moment: wait it out.
    let code = 0
    let boxes = 0
    for (let tries = 0; tries < 8; tries += 1) {
      code = Math.max(code, await codeLight())
      boxes = Math.max(boxes, await diagramLight())
      if (code > 80 || boxes > 120) break
      await sleep(250)
    }
    if (code > 80)
      failures.push(`with the OS in light mode the code block turns light (${code}/255)`)
    if (boxes > 120)
      failures.push(`with the OS in light mode the diagram turns light (${boxes}/255)`)
    if (boxes === 0) failures.push("the diagram has no filled boxes to read")
  } finally {
    await page.emulateMedia({ colorScheme: null })
  }
}

/**
 * Setup wears the app's graphite: on the welcome, its glass pane and the light
 * beside it are dark greys with no hue, as the window it hands over to is.
 * Read from the screen, since the light is drawn on a canvas. On the picker
 * the picture is the chosen agent's own (`artToo` false), and only the pane
 * is held to it.
 */
async function setupIsGraphite(page, failures, where, artToo) {
  const boxes = await page.evaluate(() =>
    [".nessa-setup-pane", ".nessa-setup-art"].map((selector) => {
      const r = document.querySelector(selector)?.getBoundingClientRect()
      // A strip down the box's middle, clear of its words and rows.
      return r && { x: r.left + 12, y: r.top + 12, w: 8, h: r.height - 24 }
    }),
  )
  const shot = (await page.screenshot()).toString("base64")
  const samples = await page.evaluate(
    async ([png, boxes, scale]) => {
      const image = new Image()
      image.src = `data:image/png;base64,${png}`
      await image.decode()
      const canvas = document.createElement("canvas")
      canvas.width = image.width
      canvas.height = image.height
      const ctx = canvas.getContext("2d")
      ctx.drawImage(image, 0, 0)
      return boxes.map((box) => {
        if (!box) return null
        let hue = 0
        let light = 0
        for (let step = 0; step <= 10; step += 1) {
          const [r, g, b] = ctx.getImageData(
            Math.round((box.x + box.w / 2) * scale),
            Math.round((box.y + (box.h * step) / 10) * scale),
            1,
            1,
          ).data
          hue = Math.max(hue, Math.max(r, g, b) - Math.min(r, g, b))
          light = Math.max(light, (r + g + b) / 3)
        }
        return { hue, light }
      })
    },
    [shot, boxes, await page.evaluate(() => window.devicePixelRatio)],
  )
  const [pane, art] = samples
  if (!pane || pane.hue > 14 || pane.light > 90)
    failures.push(`${where}: the pane is not dark graphite: ${JSON.stringify(pane)}`)
  if (artToo && (!art || art.hue > 14 || art.light > 170))
    failures.push(
      `${where}: the light beside the pane has colour: ${JSON.stringify(art)}`,
    )
}

/**
 * A summon the host reports lights the keys and lets setup be finished, and
 * sends nothing through the light behind the window: the field stays as it
 * was, settled, the whole time.
 */
async function summonedWithoutARing(page, failures, evidence) {
  const field = page.locator(".nessa-setup-field")
  await page.getByRole("button", { name: names.setup.finish[1], exact: true }).waitFor({
    timeout: 10_000,
  })
  const listening = () =>
    page.evaluate(() => window.__fakeHostListening("nessa://summoned"))
  if (!(await waitFor(async () => (await listening()) > 0, 5_000))) {
    failures.push("the summon step never listened for the host's summon")
    return
  }
  if (
    !(await waitFor(
      async () => (await field.getAttribute("data-settled")) !== null,
      5_000,
    ))
  )
    failures.push("the light behind setup never settled on the summon step")
  await field.evaluate((element) => {
    window.__setupFieldStirred = 0
    new MutationObserver(() => {
      if (!element.hasAttribute("data-settled")) window.__setupFieldStirred++
    }).observe(element, { attributes: true, attributeFilter: ["data-settled"] })
  })
  // The panel shows, then hides again: the lesson is learnt on the second.
  await page.evaluate(() => window.__fakeHostEmit("nessa://summoned", true))
  // Where a ring would be, just after the press.
  await page.waitForTimeout(300)
  if (evidence) await page.screenshot({ path: join(evidence, "onboarding-summoned.png") })
  await page.evaluate(() => window.__fakeHostEmit("nessa://summoned", false))
  const taught = await waitFor(
    async () =>
      (await page
        .getByRole("button", { name: names.setup.finish[0], exact: true })
        .count()) > 0,
    5_000,
  )
  if (!taught) failures.push("a summon the host reported did not teach the step")
  // Longer than a ring took to cross the window.
  await page.waitForTimeout(1_500)
  const stirred = await page.evaluate(() => window.__setupFieldStirred)
  if (stirred) failures.push(`a summon stirred the light behind setup ${stirred} times`)
}

/**
 * The opening was a glow on the desktop that became the window: nothing
 * dimmed what is behind setup, the beats came in order, and none of the
 * window's words showed before the window had formed.
 */
async function openedAsAGlow(page, failures) {
  const beats = await page.evaluate(() => window.__setupBeats)
  const order = beats.map((entry) => entry.beat).join(",")
  if (order !== "glow,fill,form,open")
    failures.push(`setup opened as ${order}, not glow,fill,form,open`)
  const early = beats.filter((entry) => entry.beat !== "open" && entry.headingSeen)
  if (early.length)
    failures.push(`the window's heading showed during the ${early[0].beat} beat`)
  // Glow and fill draw the window past every edge of the screen.
  for (const { beat, drawn } of beats.filter(
    (entry) => entry.beat !== "open" && entry.beat !== "form",
  ))
    if (
      drawn.left > 0 ||
      drawn.top > 0 ||
      drawn.right < drawn.width ||
      drawn.bottom < drawn.height
    )
      failures.push(
        `during ${beat} the light does not fill the screen: ${JSON.stringify(drawn)}`,
      )
  // Once open, the window has formed: its own size, in the middle.
  const formed = () =>
    page.locator(".nessa-setup-window").evaluate((shell) => {
      const box = shell.getBoundingClientRect()
      return {
        scale: box.width / shell.offsetWidth,
        dx: box.left + box.width / 2 - innerWidth / 2,
        dy: box.top + box.height / 2 - innerHeight / 2,
      }
    })
  const whole = (shape) =>
    Math.abs(shape.scale - 1) < 0.005 && Math.abs(shape.dx) < 1 && Math.abs(shape.dy) < 1
  if (!(await waitFor(async () => whole(await formed()), 3_000)))
    failures.push(
      `the open window did not form in the middle: ${JSON.stringify(await formed())}`,
    )
  const backdrop = await page.evaluate(() => ({
    dim: document.querySelectorAll(".nessa-setup-dim").length,
    sheet: getComputedStyle(document.querySelector(".nessa-setup-sheet")).backgroundColor,
  }))
  if (backdrop.dim || backdrop.sheet !== "rgba(0, 0, 0, 0)")
    failures.push(`setup dims the desktop: ${JSON.stringify(backdrop)}`)
}

/**
 * The picker lists Claude, Codex and a Local AI that cannot be chosen yet,
 * and never OpenCode. The picture wears one agent's background with that
 * agent's figure on it, the chosen agent's (Claude Code's mascot for Claude),
 * and pointing at a row turns it over to that agent.
 */
async function pickerOffers(page, failures) {
  const rows = await page
    .getByRole("group", { name: "Agent" })
    .locator(AGENT_ROWS)
    .evaluateAll((buttons) =>
      buttons.map((button) => ({
        name: button.getAttribute("aria-label") ?? button.textContent,
        disabled: button.getAttribute("aria-disabled") === "true",
      })),
    )
  const named = rows.map((row) => row.name)
  for (const agent of names.setup.agents)
    if (!named.some((name) => name === agent || name.startsWith(`${agent},`)))
      failures.push(`the picker does not list ${agent}: ${JSON.stringify(named)}`)
  const local = rows.find((row) => row.name === names.setup.localAi)
  if (!local?.disabled)
    failures.push(`Local AI is not listed as coming: ${JSON.stringify(rows)}`)
  if (named.some((name) => /opencode/i.test(name)))
    failures.push(`the picker still offers OpenCode: ${JSON.stringify(named)}`)
  // What the picture shows: the opaque background, then every figure on it.
  const cards = () =>
    page
      .locator(".nessa-setup-art")
      .evaluate((art) =>
        [
          art.querySelector("[data-slot='agent-backdrop'][data-shown]")?.dataset.agent,
          ...[...art.querySelectorAll("[data-slot='agent-figure']")].map(
            (figure) => figure.dataset.agent,
          ),
        ].join(","),
      )
  const resting = await cards()
  if (resting !== "clawd,clawd")
    failures.push(`the picture shows ${JSON.stringify(resting)}`)
  // Pointing along the rows leaves the picture as it is: only a click
  // changes it. Watched for a while, since a change would be a crossfade.
  const codex = page.getByRole("button", { name: "Codex", exact: true })
  await codex.hover()
  const moved = await waitFor(
    () => cards().then((shown) => shown !== "clawd,clawd"),
    1_000,
  )
  if (moved)
    failures.push(
      `pointing at Codex turned the picture to ${JSON.stringify(await cards())}`,
    )
  await codex.click()
  const chosen = await waitFor(
    () => cards().then((shown) => shown === "codex,codex"),
    5_000,
  )
  if (!chosen)
    failures.push(
      `clicking Codex did not turn the picture to it: ${JSON.stringify(await cards())}`,
    )
  await page.getByRole("button", { name: "Claude", exact: true }).hover()
  if (await waitFor(() => cards().then((shown) => shown !== "codex,codex"), 1_000))
    failures.push(
      `pointing at Claude after choosing Codex turned the picture to ${JSON.stringify(await cards())}`,
    )
  await page.mouse.move(1, 1)
}

/**
 * The gateway's machine probe, answered here: this stack's page is not an
 * origin it answers. `readiness` is read on every ask, so a step can change
 * what the machine has between asks.
 */
function agentsProbe(page, readiness) {
  return page.route("**/onboarding/agents", (route) =>
    route.fulfill({
      // The gateway's own origin, not the page's: the ask is cross-origin.
      headers: { "access-control-allow-origin": "*" },
      json: {
        agents: Object.entries(readiness()).map(([id, state]) => ({
          id,
          readiness: state,
        })),
      },
    }),
  )
}

/** The size the fake installer gives Codex's download. */
const CODEX_ARCHIVE_BYTES = 45_000_000

/**
 * A gateway whose installer offers Codex and installs it on request, without
 * fetching anything: the page's socket goes to the real gateway, and only
 * the installer's two asks are answered here. `installed` says when the
 * install has been asked for, so the machine probe can follow it.
 */
async function codexInstaller(page) {
  const state = { installed: false, installs: 0 }
  await page.routeWebSocket(
    () => true,
    (socket) => {
      const server = socket.connectToServer()
      const answer = (id, payload) => {
        try {
          socket.send(JSON.stringify({ type: "res", id, ok: true, payload }))
        } catch {
          // The window closed first: nothing is waiting for the answer.
        }
      }
      socket.onMessage((message) => {
        let frame
        try {
          frame = typeof message === "string" ? JSON.parse(message) : undefined
        } catch {
          // Not a request this fake answers: it goes to the gateway as sent.
        }
        if (frame?.type !== "req") frame = undefined
        if (frame?.method === "agents.installOptions")
          return answer(frame.id, {
            agents: [
              {
                agent: "codex",
                version: "0.0.0-stress",
                archiveBytes: CODEX_ARCHIVE_BYTES,
                installed: state.installed,
              },
            ],
          })
        if (frame?.method === "agents.install" && frame.params?.agent === "codex") {
          state.installs += 1
          // Long enough that the card is seen fetching.
          return void setTimeout(() => {
            state.installed = true
            answer(frame.id, {
              agent: "codex",
              version: "0.0.0-stress",
              downloaded: true,
              cleanupPending: false,
            })
          }, 600)
        }
        server.send(message)
      })
      server.onMessage((message) => socket.send(message))
    },
  )
  return state
}

/**
 * An agent that is not installed but can be downloaded is a card with one
 * Download button where its row would be, and nothing else on the picker
 * talks about downloads. Its Download installs it, and the card becomes the
 * agent's row.
 */
async function downloadCard(page, installer, evidence, failures) {
  const group = page.getByRole("group", { name: "Agent" })
  const card = group.getByRole("group", { name: names.setup.codexCard })
  const download = card.getByRole("button", {
    name: names.setup.codexDownload,
    exact: true,
  })
  const shown = await waitFor(async () => (await download.count()) === 1, 15_000)
  if (evidence)
    await page.screenshot({ path: join(evidence, "onboarding-download-card.png") })
  if (!shown) {
    failures.push("Codex, not installed but offered, is not shown as a download card")
    return
  }
  if (await group.locator("button[aria-pressed]", { hasText: "Codex" }).count())
    failures.push("Codex has both a download card and a row to pick")
  for (const text of [/Check downloads/, /Download only the agents/, /^Installed$/])
    if (await page.getByText(text).count()) failures.push(`the picker still says ${text}`)
  // Pressed from the keyboard, so where the focus goes is part of the check.
  await download.focus()
  await page.keyboard.press("Enter")
  // The installer holds its answer, so the card is seen fetching, with the
  // focus still on its button.
  const fetching = await waitFor(
    () =>
      card.evaluate(
        (each) =>
          each.dataset.downloadState === "fetching" &&
          each.contains(document.activeElement),
      ),
    2_000,
  )
  if (!fetching) failures.push("Codex's card did not fetch with the focus kept on it")
  const row = group.locator("button[aria-pressed]:not([aria-disabled='true'])", {
    hasText: "Codex",
  })
  if (!(await waitFor(async () => (await row.count()) === 1, 15_000)))
    failures.push("the installed Codex did not become a row to pick")
  else {
    if (await card.count()) failures.push("Codex's download card stayed after install")
    // The focus the download held is handed to the row it became.
    if (
      !(await waitFor(
        () => row.evaluate((each) => each === document.activeElement),
        2_000,
      ))
    )
      failures.push("the focus was not handed to Codex's row after install")
  }
  if (installer.installs !== 1)
    failures.push(`Download asked the installer ${installer.installs} times, not once`)
}

/** The picker's agent rows, and not the buttons of a key form beside one. */
const AGENT_ROWS = `button[aria-pressed], button[aria-label='${names.setup.localAi}']`

/**
 * Setup finished on a ready agent leaves by flying into the menu bar: the
 * window lets go of its pane, the host is asked where its icon is, and the
 * handoff that follows is a finish on that agent, made once the window has
 * landed and gone.
 *
 * Which agents are ready is the gateway's machine probe, which this stack's
 * page is not an origin it answers; the probe is answered here instead with
 * Claude ready, so the step is about the finish rather than about the machine.
 * Codex starts not installed, with a download on offer, so the same window
 * shows its download card on the way.
 */
async function finishedIntoTheMenuBar(run, failures) {
  let installer
  const page = await setupWindow(run, async (page) => {
    installer = await codexInstaller(page)
    await agentsProbe(page, () => ({
      claude: "ready",
      codex: installer.installed ? "ready" : "not-installed",
    }))
  })
  try {
    const button = (name) => page.getByRole("button", { name, exact: true })
    await page
      .getByRole("heading", { name: names.setup.welcome, exact: true })
      .waitFor({ timeout: 15_000 })
    await button(names.setup.getStarted).click()
    await downloadCard(page, installer, run.evidence, failures)
    const ready = page
      .getByRole("group", { name: "Agent" })
      .locator("button[aria-pressed]:not([aria-disabled='true'])")
    if (!(await waitFor(async () => (await ready.count()) > 0, 15_000))) {
      const rows = await page
        .getByRole("group", { name: "Agent" })
        .locator(AGENT_ROWS)
        .evaluateAll((all) => all.map((row) => row.getAttribute("aria-label")))
      failures.push(`no agent became ready to choose in setup: ${JSON.stringify(rows)}`)
      return
    }
    const chosen = (await ready.first().getAttribute("aria-label"))?.toLowerCase()
    await ready.first().click()
    await button(names.setup.continue).click()
    await summonedWithoutARing(page, failures, run.evidence)
    const finish = page.getByRole("button", {
      name: new RegExp(`^(${names.setup.finish.join("|")})$`),
    })
    await finish.waitFor({ timeout: 10_000 })
    await finish.click()
    const handed = await waitFor(
      async () => (await page.evaluate(() => window.__fakeHostSetupFinishes.length)) > 0,
      10_000,
    )
    const { finishes, trayAsks, left, flights, tray } = await page.evaluate(() => ({
      finishes: window.__fakeHostSetupFinishes,
      trayAsks: window.__fakeHostTrayAsks,
      left: window.__setupLeft,
      flights: window.__setupFlights,
      // Where the fake host says its icon is.
      tray: { x: innerWidth - 80, y: 12 },
    }))
    if (!left) failures.push("finishing setup did not fly the window")
    // Where the flight lands, measured from what it was given rather than
    // from its frames: a loaded headless page draws few frames, and the
    // flight's deadline can land setup before the last one. A box where the
    // window took off, wearing the last keyframe, is drawn where the window
    // ends up; its centre is on the icon, and it is a light the icon's size.
    // How long it takes on screen in a production build is measured on a Mac.
    if (flights.length !== 1)
      failures.push(`the window flew ${flights.length} times, not once`)
    else {
      const [flight] = flights
      const landing = await page.evaluate(({ from, frames }) => {
        const last = frames.at(-1)
        const probe = document.createElement("div")
        Object.assign(probe.style, {
          position: "fixed",
          left: `${from.left}px`,
          top: `${from.top}px`,
          width: `${from.width}px`,
          height: `${from.height}px`,
          transform: last.transform,
        })
        document.body.append(probe)
        const box = probe.getBoundingClientRect()
        probe.remove()
        return {
          x: box.left + box.width / 2,
          y: box.top + box.height / 2,
          width: box.width,
          height: box.height,
          opacity: Number(last.opacity),
          round: frames.slice(1).every((frame) => frame.borderRadius === "50%"),
          offset: last.offset,
        }
      }, flight)
      if (
        Math.abs(landing.x - tray.x) > 1 ||
        Math.abs(landing.y - tray.y) > 1 ||
        Math.abs(landing.width - 17.6) > 0.5 ||
        Math.abs(landing.height - 17.6) > 0.5 ||
        landing.opacity !== 0 ||
        !landing.round ||
        flight.duration !== 900
      )
        failures.push(
          `the flight lands ${JSON.stringify({ ...landing, duration: flight.duration })}, not a round 17.6 px light on ${JSON.stringify(tray)} over 900 ms`,
        )
    }
    if (!handed) failures.push("finishing setup did not hand over to the host")
    else if (
      finishes.length !== 1 ||
      finishes[0].completed !== true ||
      finishes[0].agent !== chosen
    )
      failures.push(
        `finishing handed over ${JSON.stringify(finishes)}, not one finish on ${chosen}`,
      )
    else if (finishes[0].windowShown)
      failures.push("setup handed over while its window was still on screen")
    if (trayAsks !== 1)
      failures.push(
        `the flight asked where the menu bar icon is ${trayAsks} times, not once`,
      )
  } finally {
    await page.close()
  }
}

/**
 * Where setup's corner buttons are, against its window: which buttons are
 * drawn, and for each its centre as fractions of the window's width and
 * height, and whether it is round.
 */
function setupCorners(page) {
  return page.evaluate((labels) => {
    const frame = document.querySelector(".nessa-setup-window")?.getBoundingClientRect()
    const at = (label) => {
      const button = document.querySelector(`button[aria-label="${label}"]`)
      if (!frame || !button) return null
      const box = button.getBoundingClientRect()
      return {
        x: (box.left + box.width / 2 - frame.left) / frame.width,
        y: (box.top + box.height / 2 - frame.top) / frame.height,
        round:
          Math.abs(box.width - box.height) < 0.5 &&
          parseFloat(getComputedStyle(button).borderRadius) >= box.width / 2 - 0.5,
      }
    }
    return { back: at(labels.back), close: at(labels.close) }
  }, names.setup)
}

/** Whether a corner button sits in the window's top-left or top-right corner. */
function inCorner(place, side) {
  if (!place || !place.round || place.y > 0.15) return false
  return side === "left" ? place.x < 0.15 : place.x > 0.85
}

/**
 * Nessa's own shell tool as the gateway's managed `nessa` server: the
 * `nessa-mcp` built beside the gateway, working in a directory of the run's.
 */
function shellServer(directory) {
  const gateway = process.env.MCP_LIVE_NESSA ?? join(repoRoot, "target/debug/nessa")
  const command = join(dirname(gateway), "nessa-mcp")
  if (!existsSync(command))
    throw new CannotRun(
      `build the shell tool first (cargo build -p nessa-mcp): ${command}`,
    )
  const workspace = join(directory, "shell-workspace")
  mkdirSync(workspace)
  return {
    name: "nessa",
    command,
    args: ["--workspace", workspace, "--audit-directory", join(directory, "shell-audit")],
  }
}

/**
 * A new conversation's agent, opened now, says what GitHub sign-in it was
 * started with; `said` is what it must say. A sign-in is read when an agent
 * opens, so only a new one shows a change.
 */
async function githubAgent(run, step, extra, said, failures, shell = false) {
  const conversationId = randomUUID()
  run.checked.push(conversationId)
  const text = prompt(run, shell ? STRESS.githubShell : STRESS.github, step, extra)
  await run.client.conversation.create({ conversationId, agent: run.stack.agent })
  await run.client.conversation.submit(
    run.client.conversation.prepare(conversationId, text),
  )
  const { turn } = await settled(
    run,
    conversationId,
    new Set(),
    text,
    "completed",
    failures,
  )
  if (!turn) return
  const answer = (turn.parts ?? []).map((part) => part.text ?? "").join("")
  const where = shell ? SAYS.githubShell : SAYS.github
  if (answer !== `${where}${said}`)
    failures.push(
      `an agent started${extra} says ${JSON.stringify(answer)}${shell ? " from its shell tool" : ""}, not ${said}`,
    )
}

/** The gateway uses its sign-in for a bounded repository GET, then returns no token. */
async function githubReadAgent(run, step, failures) {
  const text = prompt(run, STRESS.githubRead, step, " connected")
  const seen = await send(run, run.main.id, text, failures)
  const { turn } = await settled(run, run.main.id, seen, text, "completed", failures)
  if (!turn) return
  const answer = (turn.parts ?? []).map((part) => part.text ?? "").join("")
  if (answer !== `${SAYS.githubRead}nessalabs/nessa-agent`)
    failures.push(`the brokered repository read says ${JSON.stringify(answer)}`)
  await expectShown(run.page, `${SAYS.githubRead}nessalabs/nessa-agent`, failures)
}

/** Leaves Settings: Esc does not close it; its back control returns to the app. */
async function closeSettings(page) {
  await page.getByRole("button", { name: "Back to nessa Studio" }).first().click()
  await page.waitForSelector(css.settings, { state: "detached", timeout: 10_000 })
}

/**
 * With linking on, the gateway is found on the local network the way a phone
 * that scanned the code finds it: a DNS-SD browse for `_nessa-link._tcp`
 * hears it on its native port, with its gateway id. Resolves whether it did.
 */
async function announced(run, failures) {
  const port = Number(run.nativeListen.split(":").pop())
  let heard = []
  for (
    let attempt = 0;
    attempt < 3 && !heard.some((each) => each.port === port);
    attempt++
  )
    heard = await browse("_nessa-link._tcp.local", 3_000)
  const gateway = heard.find((each) => each.port === port)
  if (!gateway) {
    failures.push(
      `the gateway was not announced on port ${port}; heard ${JSON.stringify(heard)}`,
    )
    return false
  }
  if (!gateway.txt.gw) failures.push("the announcement carries no gateway id")
  return Boolean(gateway.txt.gw)
}

/**
 * Pair a device on the Linked devices tab, with linking on: the window's own
 * sign-in opens a code, shown as a person pairing a phone sees it, never the
 * card that says this sign-in cannot manage credentials. The code opens in a
 * modal with the one signal orb, none of it drawn on the page; Escape hides
 * the modal and leaves the code open, and Show code brings back the same
 * code. The code enrolls the owner's phone with full access: the gateway
 * holds it as that class. Cancel in the modal then closes it and Pair a
 * device is offered again. Resolves whether all of that held.
 */
/** The class of a code that enrolls the owner's phone with full access. */
const OWNER_PHONE_CLASS = "gateway-owner-conversation-read-write"

async function pairFromWindow(run, failures) {
  const { page } = run
  const before = failures.length
  const dialog = page.locator(css.pairingDialog)
  const codeIn = async () =>
    (await dialog.locator('[data-slot="pairing-code-value"]').innerText()).replace(
      /\s+/g,
      "",
    )
  await page.locator(selectorFor.linkedAction("pair")).click()
  const shown = await waitFor(
    async () =>
      (await page.locator(css.linkedRefused).count()) > 0
        ? "refused"
        : (await dialog.locator(css.pairingCode).count()) > 0
          ? "code"
          : null,
    10_000,
  )
  if (shown !== "code") {
    failures.push(
      shown === "refused"
        ? "Pair a device was refused: the window's sign-in cannot manage credentials"
        : "Pair a device opened no modal with a code",
    )
    return false
  }
  // The orb gathers in, then stays live: animated, not drawn still.
  const orb = dialog.locator(css.signalOrb)
  const live = await waitFor(
    async () => (await orb.getAttribute("data-state").catch(() => null)) === "live",
    5_000,
  )
  if (!live)
    failures.push(
      `the pairing orb was ${await orb.getAttribute("data-state").catch(() => "missing")}, not live`,
    )
  if (run.evidence)
    await page
      .screenshot({ path: join(run.evidence, `pairing-${run.engine}-modal.png`) })
      .catch(() => {})
  const code = await codeIn()
  if (!code) failures.push("the pairing code shown was empty")
  // A phone paired from the window acts as the owner, with no share per chat.
  const classes = await run.client.pairing.pending().then(
    ({ items }) => items.map((item) => item.class),
    (error) => [String(error)],
  )
  if (classes.length !== 1 || classes[0] !== OWNER_PHONE_CLASS)
    failures.push(
      `the open code enrolls ${JSON.stringify(classes)}, not the owner's phone`,
    )
  // Settings is a fixed layer over the window: a modal portalled past it is
  // in the page but drawn underneath it, unseen.
  if (!(await dialog.evaluate((node) => Boolean(node.closest("[data-settings-layers]")))))
    failures.push("the pairing modal is not inside Settings, so it is drawn under it")
  const orbs = await dialog.locator(css.signalOrb).count()
  if (orbs !== 1) failures.push(`the pairing modal drew ${orbs} signal orbs, not 1`)
  if ((await page.locator(css.qrOrb).count()) > 0) failures.push("a QR orb was drawn")
  if ((await page.locator(`${css.linked} ${css.pairingCode}`).count()) > 0)
    failures.push("the pairing code was drawn on the tab, not only in the modal")
  await page.keyboard.press("Escape")
  await dialog
    .waitFor({ state: "detached", timeout: 10_000 })
    .catch(() => failures.push("Escape did not close the pairing modal"))
  await page.locator(selectorFor.linkedAction("show")).click()
  await dialog
    .waitFor({ timeout: 10_000 })
    .catch(() => failures.push("Show code did not open the pairing modal again"))
  const again = await codeIn().catch(() => "")
  if (again !== code)
    failures.push(`Show code showed ${JSON.stringify(again)}, not the open code`)
  if (failures.length !== before) return false
  // A device enters the code the way the phone does, is approved in this
  // modal, ends paired, and is revoked again from the list.
  if (!(await phonePairs(run, code, failures))) return false
  // A second code is cancelled from its modal.
  await page.locator(selectorFor.linkedAction("pair")).click()
  await dialog
    .locator(css.pairingCode)
    .waitFor({ timeout: 10_000 })
    .catch(() => failures.push("Pair a device opened no second code"))
  await dialog.locator(selectorFor.linkedAction("cancel")).click()
  await dialog
    .waitFor({ state: "detached", timeout: 10_000 })
    .catch(() => failures.push("the cancelled code's modal stayed open"))
  await page
    .locator(selectorFor.linkedAction("pair"))
    .waitFor({ timeout: 10_000 })
    .catch(() => failures.push("Pair a device was not offered again after cancelling"))
  return failures.length === before
}

/**
 * Pair a real device with the open `code`, the way the phone does: the
 * native client (`read_only_sync pair`, the device class an owner's phone
 * code accepts) redeems it on the native port, its key shows in the modal,
 * and Approve there makes it active. The client must read its own active
 * status, which is where a phone holding an older wire contract failed while
 * the window already listed it. Approve keeps its primary fill on hover. The
 * device is then listed as the owner's phone, Pair a device stays offered
 * through the tab's quiet polls, and Revoke drops it with a "Revoked." that a
 * later poll clears. Resolves whether all of that held. The modal is closed
 * and nothing is left open.
 */
async function phonePairs(run, code, failures) {
  const { page } = run
  const before = failures.length
  const dialog = page.locator(css.pairingDialog)
  const root = mkdtempSync(join(tmpdir(), "nessa-stress-phone-"))
  const stateRoot = join(root, "state")
  const cache = join(root, "cache")
  mkdirSync(stateRoot, { mode: 0o700 })
  mkdirSync(cache, { mode: 0o700 })
  const profile = join(root, "profile.json")
  writeFileSync(
    profile,
    JSON.stringify({
      stateRoot,
      stateDirectory: "device",
      cache,
      gatewayAddress: run.nativeAddress,
    }),
    { mode: 0o600 },
  )
  const child = spawn(peerExampleBin(), ["pair", profile], {
    stdio: ["pipe", "pipe", "pipe"],
  })
  let stdout = ""
  let stderr = ""
  child.stdout.setEncoding("utf8")
  child.stderr.setEncoding("utf8")
  child.stdout.on("data", (chunk) => (stdout += chunk))
  child.stderr.on("data", (chunk) => (stderr += chunk))
  const exited = new Promise((resolve) => child.once("exit", resolve))
  child.stdin.end(`${code}\n`)
  try {
    const approve = dialog.locator(selectorFor.linkedAction("approve"))
    const claimed = await approve
      .waitFor({ timeout: 30_000 })
      .then(() => true)
      .catch(() => false)
    if (!claimed) {
      failures.push(
        `the device's key never showed in the pairing modal: ${stderr.trim() || stdout.trim()}`,
      )
      return false
    }
    if (await approve.isDisabled()) failures.push("Approve was drawn disabled")
    // Hovered, Approve keeps a primary fill under its dark label: it must
    // not take the plain button's faint hover, which Deny shows.
    // Both are read once their fill has stopped moving: mid-transition, any
    // two buttons can differ.
    const deny = dialog.locator(selectorFor.linkedAction("deny"))
    await deny.hover()
    const plain = await restingFill(deny)
    await approve.hover()
    const fill = await restingFill(approve)
    if (fill === plain)
      failures.push(`hovered Approve has the plain button's fill ${plain}`)
    if (run.evidence)
      await page
        .screenshot({ path: join(run.evidence, `pairing-${run.engine}-approve.png`) })
        .catch(() => {})
    await approve.click()
    const status = await Promise.race([exited, sleep(30_000).then(() => "timeout")])
    let enrollment = peerOutcome(stdout)?.enrollment
    if (status === "timeout")
      failures.push(`the device's pairing did not finish: ${stderr.trim()}`)
    else {
      // `pair` can return once its claim is confirmed, before the approval
      // has activated it: ask the device again until it reads itself active.
      let read = { stderr: "" }
      for (let tries = 0; enrollment?.phase !== "active" && tries < 40; tries += 1) {
        if (tries > 0) await sleep(250)
        read = peerExample(["status", profile])
        enrollment = read.outcome?.enrollment ?? enrollment
      }
      if (enrollment?.phase !== "active")
        failures.push(
          `the device did not read itself paired (exit ${status}, ${enrollment?.phase ?? "no status"}): ${read.stderr || stderr.trim()}`,
        )
    }
    if (failures.length !== before) return false
    await page.keyboard.press("Escape")
    await dialog.waitFor({ state: "detached", timeout: 10_000 }).catch(() => {})
    const row = page.locator(
      `${css.linked} [data-credential="${enrollment.credentialId}"]`,
    )
    const listed = await row
      .waitFor({ timeout: 15_000 })
      .then(() => true)
      .catch(() => false)
    if (!listed) {
      failures.push("the paired device was not listed under Your devices")
      return false
    }
    if (!(await row.innerText()).includes(OWNER_PHONE_LABEL))
      failures.push(
        `the paired device is not listed as ${JSON.stringify(OWNER_PHONE_LABEL)}`,
      )
    if (!(await pairOfferedThroughPolls(run, failures))) return false
    await row.locator(selectorFor.linkedAction("revoke")).click()
    await row.locator(selectorFor.linkedAction("confirm-revoke")).click()
    await row
      .waitFor({ state: "detached", timeout: 15_000 })
      .catch(() => failures.push("the revoked device stayed listed"))
    const notice = page.locator(`${css.linked} [data-linked-notice]`)
    const said = await waitFor(
      async () =>
        ((await notice.innerText().catch(() => "")) || "").startsWith("Revoked."),
      5_000,
    )
    if (!said) failures.push("Revoke said nothing")
    // The next quiet poll has read the list without it: the line goes.
    const spent = await waitFor(async () => (await notice.count()) === 0, 15_000)
    if (!spent)
      failures.push(`"Revoked." stayed under the page after the list was read again`)
    return failures.length === before
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL")
    await exited
    rmSync(root, { recursive: true, force: true })
  }
}

/** A button's background once two reads 200 ms apart agree. */
async function restingFill(button) {
  const read = () => button.evaluate((node) => getComputedStyle(node).backgroundColor)
  let last = await read()
  for (let tries = 0; tries < 20; tries++) {
    await sleep(200)
    const now = await read()
    if (now === last) return now
    last = now
  }
  return last
}

/** How the window lists a phone paired with a code from the window. */
const OWNER_PHONE_LABEL = "Your phone · full access"

/**
 * Pair a device stays offered while the tab sits: its quiet poll, every few
 * seconds, is not a request the person waits on, so it does not grey the
 * button out. Sampled across more than one poll.
 */
async function pairOfferedThroughPolls(run, failures) {
  const pair = run.page.locator(selectorFor.linkedAction("pair"))
  const offered = await pair
    .waitFor({ timeout: 15_000 })
    .then(() => true)
    .catch(() => false)
  if (!offered) {
    failures.push("Pair a device was not offered once the device was paired")
    return false
  }
  const end = Date.now() + 9_000
  while (Date.now() < end) {
    if (await pair.isDisabled()) {
      failures.push("Pair a device greyed out while the tab only polled")
      return false
    }
    await sleep(100)
  }
  return true
}

/**
 * Turns native linking on from Devices › Add a computer, with linking off:
 * its "Let other computers reach it" button, which starts the gateway again
 * in the same process. The page then makes a code, the button is gone, the
 * run's client is connected again, and config.json holds linking on. Leaves
 * the side rail closed. Called with nothing else in flight
 * (`run.exclusive`).
 */
/**
 * Devices › Thread order, as a person uses it: the side rail's Devices,
 * Thread order, then the computer's switch. Done once the gateway's own
 * read of the pool says `environment` is let in or left out; the window is
 * left on the main conversation.
 */
async function setPoolInclusion(run, environment, included, failures) {
  const page = run.page
  await setSideRail(page, true, failures)
  if (failures.length) return false
  await page.locator('[data-rail-item="devices"]').click()
  await page.locator('[data-devices-action="pool"]').click({ timeout: 15_000 })
  const row = page.locator(`[data-devices-pool-row="${environment}"]`)
  if (!(await waitFor(async () => (await row.count()) === 1, 10_000))) {
    failures.push(`Thread order lists no row for ${environment}`)
    return false
  }
  if ((await row.getAttribute("data-included")) !== String(!included))
    failures.push(`${environment} is already ${included ? "in" : "left out"}`)
  await page.locator(`[data-devices-pool-include="${environment}"]`).click()
  const saved = await waitFor(async () => {
    const pool = await run.client.devices.pool()
    return pool.defaultPreference.excluded.includes(environment) === !included
      ? pool
      : null
  }, 10_000)
  if (!saved) {
    failures.push(`the pool never saved ${environment} ${included ? "in" : "left out"}`)
    return false
  }
  if (
    !(await waitFor(
      async () => (await row.getAttribute("data-included")) === String(included),
      5_000,
    ))
  )
    failures.push(`Thread order still draws ${environment} as it was`)
  if (run.evidence)
    await page
      .screenshot({
        path: join(
          run.evidence,
          `pool-order-${run.engine}-${included ? "in" : "out"}.png`,
        ),
      })
      .catch(() => {})
  await page.locator('[data-devices-action="back"]').click()
  // Back to the chats, and the rail off again as the run had it.
  await page.locator('[data-rail-item="agents"]').click()
  await setSideRail(page, false, failures)
  await openStressConversation(run.page, run.main.id, run.main.title)
  return failures.length === 0
}

async function reachFromDevices(run, failures) {
  const gateway = run.stack.gateway
  const config = join(gateway.directory, "ci", "instances", INSTANCE, "config.json")
  const pid = gateway.server.pid
  const page = run.page
  await setSideRail(page, true, failures)
  if (failures.length) return false
  await page.locator('[data-rail-item="devices"]').click()
  await page.locator('[data-devices-action="add"]').click()
  await page.locator('[data-devices-add="computer"]').click()
  const add = page.locator('[data-devices-page="add"]')
  const reach = add.locator('[data-devices-action="reach"]')
  if (!(await waitFor(async () => (await reach.count()) === 1, 10_000))) {
    failures.push("Add a computer offers no way to let others reach the hub")
    return false
  }
  // The way to turn linking on sits with the code it lets the hub make.
  if (
    (await add
      .locator("section", {
        has: page.locator("h2", { hasText: "On the computer, run" }),
      })
      .locator('[data-devices-action="reach"]')
      .count()) !== 1
  )
    failures.push("Let other computers reach it is not beside the code")
  let held = false
  await run.restartConsole.during(run.opened, async () => {
    const before = failures.length
    const old = run.client
    run.restarts += 1
    await reach.click()
    const made = await waitFor(
      async () =>
        (await reach.count()) === 0 &&
        (await add.locator("[data-devices-command]").count()) === 2,
      60_000,
    )
    if (!made)
      failures.push(
        `after reaching, Add a computer reads ${JSON.stringify(await add.innerText())}`,
      )
    old.close()
    run.client = await connectClient(gateway, INSTANCE, "panel")
    const saved = JSON.parse(readFileSync(config, "utf8")).native
    if (saved?.enabled === false || saved?.listenAddress !== run.nativeListen)
      failures.push(`config.json holds native ${JSON.stringify(saved)}`)
    const { listening } = await run.client.devices.list()
    if (listening === undefined) failures.push("devices.list names no listener")
    if (gateway.server.pid !== pid || gateway.server.exitCode !== null)
      failures.push("the gateway did not start again in the same process")
    held = failures.length === before
    return held
  })
  await add.locator('[data-devices-action="back"]').click()
  await page.locator('[data-rail-item="agents"]').click()
  await setSideRail(page, false, failures)
  return held
}

/**
 * Turns native linking `enabled` with the switch on the Linked devices tab,
 * already open. The gateway saves config.json and starts itself again in
 * the same process; the window comes back to the new state, and the run's
 * client is connected again. Resolves whether all of that held. Called with
 * nothing else in flight (`run.exclusive`).
 */
async function switchLinking(run, enabled, failures) {
  const gateway = run.stack.gateway
  const config = join(gateway.directory, "ci", "instances", INSTANCE, "config.json")
  const pid = gateway.server.pid
  const want = enabled ? "on" : "off"
  const before = await waitFor(
    async () =>
      (await run.page.locator(css.linked).getAttribute("data-linked")) ===
      (enabled ? "off" : "on"),
    30_000,
  )
  if (!before) {
    failures.push(`linking was not ${enabled ? "off" : "on"} before the switch`)
    return false
  }
  let held = false
  await run.restartConsole.during(run.opened, async () => {
    const before = failures.length
    const old = run.client
    run.restarts += 1
    await run.page.locator(css.linkingSwitch).click()
    const shown = await waitFor(
      async () =>
        (await run.page.locator(css.linked).getAttribute("data-linked")) === want,
      60_000,
    )
    if (!shown) failures.push(`the window did not come back with linking ${want}`)
    old.close()
    run.client = await connectClient(gateway, INSTANCE, "panel")
    const answered = await run.client.pairing.pending().then(
      () => "on",
      (error) => (error?.refusal === "pairing_not_configured" ? "off" : String(error)),
    )
    if (answered !== want)
      failures.push(`the gateway answers linking ${answered}, not ${want}`)
    const saved = JSON.parse(readFileSync(config, "utf8")).native
    if (
      (saved?.enabled !== false) !== enabled ||
      saved?.listenAddress !== run.nativeListen
    )
      failures.push(`config.json holds native ${JSON.stringify(saved)}`)
    if (gateway.server.pid !== pid || gateway.server.exitCode !== null)
      failures.push("the gateway did not start again in the same process")
    held = failures.length === before
    return held
  })
  return held
}

/**
 * The places the gateway offers a new conversation (`agents.list`
 * `environments`), once `holds` is true of them; what they last were after
 * 30 s otherwise. A worker that connects or leaves changes them.
 */
/**
 * The `tunnel` step's stand-in cloudflared (`lib/fake-cloudflared.mjs`),
 * written into `directory` with this node as its interpreter: the gateway
 * starts it with no PATH, and one exec keeps the name it records.
 */
function fakeCloudflared(directory) {
  const source = readFileSync(
    new URL("./lib/fake-cloudflared.mjs", import.meta.url),
    "utf8",
  )
  const program = join(directory, "cloudflared")
  writeFileSync(program, `#!${process.execPath}\n${source.replace(/^#!.*\n/, "")}`)
  chmodSync(program, 0o755)
  return program
}

/** A direct address that refuses at once, so a dial goes beyond it. */
const UNREACHABLE = "127.0.0.1:9"

/** The gateway's `<namespace>/tunnel`: the kept record and the program's log. */
function tunnelDirectory(run) {
  return join(run.stack.gateway.directory, "ci", "instances", INSTANCE, "tunnel")
}

/** The cloudflared shepherd keeps for the tunnel, as `{ pid }` from its record; `null` without one. */
function keptTunnel(directory) {
  try {
    const record = JSON.parse(
      readFileSync(join(directory, "kept", "cloudflared.json"), "utf8"),
    )
    return Number.isInteger(record.os_pid) ? { pid: record.os_pid } : null
  } catch {
    return null
  }
}

/** Connections the stand-in cloudflared has carried, as its log counts them. */
function tunnelConnections(directory) {
  try {
    return readFileSync(join(directory, "cloudflared.log"), "utf8")
      .split("\n")
      .filter((line) => /^connection \d+$/.test(line)).length
  } catch {
    return 0
  }
}

function alive(pid) {
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    return error.code === "EPERM"
  }
}

/**
 * Turns the web tunnel `enabled` with its switch on the Linked devices tab,
 * already open with linking on. The gateway saves the change and starts
 * itself again in the same process; the window comes back showing the
 * tunnel ready, or off. Resolves whether all of that held. Called with
 * nothing else in flight (`run.exclusive`).
 */
async function switchTunnel(run, enabled, failures) {
  const gateway = run.stack.gateway
  const pid = gateway.server.pid
  const want = enabled ? "ready" : "off"
  let held = false
  await run.restartConsole.during(run.opened, async () => {
    const before = failures.length
    const old = run.client
    run.restarts += 1
    await run.page.locator(css.tunnelSwitch).click()
    const shown = await waitFor(async () => {
      const group = run.page.locator(css.tunnel)
      return (
        (await group.getAttribute("data-tunnel-enabled")) === String(enabled) &&
        (await group.getAttribute("data-tunnel-status")) === want
      )
    }, 60_000)
    if (!shown) failures.push(`the window did not come back with the tunnel ${want}`)
    else if (enabled && (await run.page.locator(css.tunnelAddress).count()) !== 1)
      failures.push("the ready tunnel's address is not shown")
    old.close()
    run.client = await connectClient(gateway, INSTANCE, "panel")
    const read = await run.client.pairing.tunnel()
    if (read.enabled !== enabled || read.status !== want)
      failures.push(`the gateway answers the tunnel ${JSON.stringify(read)}`)
    if (gateway.server.pid !== pid || gateway.server.exitCode !== null)
      failures.push("the gateway did not start again in the same process")
    held = failures.length === before
    return held
  })
  return held
}

/**
 * The computers among what the gateway offers, in its order (the pool
 * first): SSH hosts an earlier step connected are offered before them.
 */
const computers = (offered) => offered?.filter((name) => name.startsWith("peer:")) ?? []

const offers = (run, holds) =>
  waitFor(async () => {
    const { environments } = await run.client.agents.list()
    return holds(environments) ? environments : null
  }, 30_000).then(
    (environments) =>
      environments ?? run.client.agents.list().then((each) => each.environments),
  )

/** The gateway's conversations directory, where it keeps place records. */
const conversationsOf = (run) =>
  join(run.stack.gateway.directory, "ci", "instances", INSTANCE, "conversations")

/**
 * Which worker of the pool holds `conversationId` now, as the gateway keeps
 * it: its place record, `places/<id>.json`, or an older build's
 * `bindings/<id>.json`, as `{ worker, context }`. `null` when it has none.
 */
function bindingOf(run, conversationId) {
  const place = placeRecordOf(conversationsOf(run), conversationId)
  return place?.worker ? { worker: place.worker, context: place.context } : null
}

/** What the window's heading names `worker` by, in the client's words. */
async function placeOf(worker) {
  const { environmentPlace } = await import("@nessa/client")
  return environmentPlace(worker.name)
}

/**
 * The first view frame of `conversationId` from a view subscription, which
 * never opens an agent. Closed at once: a follower keeps its agent from
 * being parked.
 */
async function followedOnce(run, conversationId) {
  let seen
  let lost
  const first = new Promise((resolve, reject) => ((seen = resolve), (lost = reject)))
  const opening = run.client.subscriptions.view(conversationId, {
    view: (each) => seen(each.view),
    ended: (end) => lost(new Error(`the view ended ${end.reason}`)),
  })
  try {
    return await Promise.race([
      first,
      opening.then(() => first),
      sleep(15_000).then(() => {
        throw new Error("no view frame in 15 s")
      }),
    ])
  } finally {
    // Closed whatever happened, so nothing follows it afterwards; an open
    // that failed left nothing to close.
    const subscription = await opening.catch(() => null)
    await subscription?.close().catch(() => {})
  }
}

/**
 * What a watch turn's reads came to, as the agent said it: of `WATCH_CALLS`
 * at once, some were refused for want of room and the rest waited, and
 * nothing else happened to any of them.
 */
function watched(turn, failures) {
  const said = (turn.parts ?? []).map((part) => part.text ?? "").join("")
  const at = said.indexOf(SAYS.watched)
  let results
  try {
    results = JSON.parse(said.slice(at + SAYS.watched.length))
  } catch {
    failures.push(
      `the watch turn did not say what its reads came to: ${said.slice(0, 120)}`,
    )
    return
  }
  if (!Array.isArray(results) || results.length !== WATCH_CALLS) {
    failures.push(`the watch turn made ${results?.length} reads, not ${WATCH_CALLS}`)
    return
  }
  const refused = results.filter((each) => each?.error)
  const waited = results.filter((each) => !each?.error)
  // The agent's own server takes only so many calls at once, and of those
  // the conversation only so many waiting reads: both refusals are expected.
  const other = refused.filter(
    (each) => !["subscription_capacity", "call_refused"].includes(each.error),
  )
  if (other.length)
    failures.push(`reads were refused ${[...new Set(other.map((each) => each.error))]}`)
  if (!refused.some((each) => each.error === "subscription_capacity"))
    failures.push(`no read of ${WATCH_CALLS} at once was refused for want of room`)
  if (!waited.length) failures.push("no read at all was taken")
  if (waited.some((each) => each.timedOut !== true && !each.events?.length))
    failures.push("a read that was taken neither waited nor returned events")
}

/**
 * One background conversation: `turns` turns through the gateway, a reply
 * then a permission it allows, over and over. Waits while a restart holds
 * the gateway. Resolves with what went wrong.
 */
async function background(run, n) {
  const failures = []
  const conversationId = randomUUID()
  try {
    await run.enter()
    try {
      await run.client.conversation.create({ conversationId, agent: run.stack.agent })
    } finally {
      run.leave()
    }
    for (let turn = 0; turn < run.knobs.turns; turn += 1) {
      const permission = turn % 3 === 1
      const watch = turn % 3 === 2
      const phrase = watch ? STRESS.watch : permission ? STRESS.permission : STRESS.reply
      const text = `${phrase} b${n}.${turn} s${run.knobs.seed}`
      await run.enter()
      try {
        await run.client.conversation.submit(
          run.client.conversation.prepare(conversationId, text),
        )
        const { permissionTargets } = await import("@nessa/client")
        const answered = new Set()
        const { turn: done } = await untilTurn(
          run,
          conversationId,
          { text },
          (current, view) => {
            const waiting = view.permissions[0]
            if (waiting && !answered.has(waiting.review)) {
              answered.add(waiting.review)
              const option = waiting.options.find((each) => each.effect === "allow")
              const restarts = run.restarts
              void run.client.conversation
                .answer(permissionTargets(view)[0], option.id)
                .catch((error) => {
                  // The socket's control lanes were full, so nothing was
                  // applied; the code says to send it again, as a client must.
                  if (error?.code === "temporarily_unavailable") {
                    answered.delete(waiting.review)
                    return
                  }
                  // A restart can drop the answer's acknowledgement, not the
                  // answer: the turn must still complete, checked below.
                  if (run.restarts === restarts)
                    failures.push(
                      `background ${n} at ${new Date().toISOString()}: answer: ${explain(error)}`,
                    )
                })
            }
            return ended(current)
          },
          120,
        )
        if (done?.status !== "completed")
          failures.push(
            `background ${n} turn ${turn} ended ${done?.status ?? "missing"}${done?.error ? ` (${done.error})` : ""}`,
          )
        else if (watch) watched(done, failures)
      } finally {
        run.leave()
      }
    }
  } catch (error) {
    failures.push(`background ${n} at ${new Date().toISOString()}: ${explain(error)}`)
  }
  return { conversationId, failures }
}

/** The app server the scripted agent's tools come from. */
const TEST_APP_SERVER = "mcptest"

/**
 * Whether the gateway keeps `server` trusted: its
 * `<namespace>/conversations/trusted-apps.json`, `{"servers": [...]}`.
 */
function appTrusted(run, server) {
  const file = join(
    run.stack.gateway.directory,
    "ci",
    "instances",
    INSTANCE,
    "conversations",
    "trusted-apps.json",
  )
  try {
    return (JSON.parse(readFileSync(file, "utf8")).servers ?? []).includes(server)
  } catch {
    return false
  }
}

/**
 * The `huddle` step in the window: the talk board a voice app stands in
 * for. Its frame is allowed the microphone because its resource asks; it
 * opens in the chat's panel as one tab however often it asks, its card
 * becoming a row; its first message is reviewed, offering Always allow this
 * app, and once that is chosen its next message goes unasked. Trust is kept
 * gateway-wide by server; each repetition ends by revoking it from Settings,
 * so a later repetition is asked about again.
 */
async function talkBoard(run, step, conversationId, failures, shot) {
  const page = run.page
  const live = (frame) =>
    frame
      .waitForSelector('body[data-talk-state="live"]', { timeout: 20_000 })
      .then(() => true)
      .catch(() => false)
  let inline
  try {
    inline = await appFrame(page, "inline", 20_000)
  } catch {
    failures.push("the talk board's turn drew no app")
    return
  }
  if (!(await live(inline.app))) {
    failures.push("the talk board never went live inline")
    return
  }
  const allow = await (await inline.app.frameElement()).getAttribute("allow")
  if (allow !== "microphone *")
    failures.push(
      `the talk board's frame allows ${JSON.stringify(allow)}, not the microphone`,
    )
  await shot("inline")
  // In the chat's panel, one tab, and the card gives way to a row.
  const tabs = page.locator(`${css.chatPanelTabs} [title^="Close "]`)
  const tabsBefore = await tabs.count()
  const sidePanel = page.locator("#workspace-panel")
  const panelShown = async () => (await sidePanel.getAttribute("aria-hidden")) === null
  const panelWasShown = await panelShown()
  await inline.app.click('[data-talk="panel"]')
  let panel
  try {
    panel = await appFrame(page, "panel", 20_000)
  } catch {
    failures.push("the talk board's fullscreen request opened no panel tab")
    return
  }
  if (!(await live(panel.app))) {
    failures.push("the talk board never went live in the panel")
    return
  }
  const rowOnly = await waitFor(
    async () => !(await page.$(selectorFor.appFrameIn("inline"))),
    5_000,
  )
  if (!rowOnly)
    failures.push("the talk board is drawn inline while it is open in the panel")
  // Its panel mount went live and the card stayed a row, so an unsized
  // `/mcp-resources` abort from the gateway that serves it is the delivered
  // fetch (#473). Every other abort stays a failure.
  else noteLiveMountResourceAbort(run.opened, "live", run.stack.gateway.url)
  await panel.app.click('[data-talk="panel"]')
  await sleep(1_000)
  const opened = (await tabs.count()) - tabsBefore
  if (opened !== 1) failures.push(`the talk board opened ${opened} panel tabs, not one`)
  await shot("panel")
  // A message as the person: asked about once, then Always allow.
  const ask = async (text) => {
    await panel.app.fill("#text", text)
    await panel.app.click('[data-talk="send"]')
  }
  const sent = (ms) =>
    waitFor(async () => {
      const said = await panel.app.evaluate(
        () => document.getElementById("sent")?.textContent ?? "",
      )
      return said !== "" && said !== "pending" ? said : null
    }, ms)
  const firstText = `${STRESS.reply} from the talk board r${step.index} first`
  const card = page.locator(css.appApprovalCard).filter({ visible: true }).first()
  const trustedBefore = appTrusted(run, TEST_APP_SERVER)
  await ask(firstText)
  if (trustedBefore) {
    // Every repetition revokes its Always allow from Settings at its end
    // (`untrustFromSettings`), so this one should be asked again.
    failures.push(
      "the talk board was still trusted after an earlier repetition revoked it",
    )
    const firstSaid = await sent(30_000)
    if (firstSaid !== "ok")
      failures.push(
        `the already trusted talk board's first message ended ${JSON.stringify(firstSaid)}`,
      )
    if (await card.isVisible().catch(() => false))
      failures.push("the already trusted talk board's first message was reviewed")
  } else {
    const always = card.getByRole("button", { name: /always allow/i })
    const offered = await always
      .waitFor({ state: "visible", timeout: 20_000 })
      .then(() => true)
      .catch(() => false)
    if (!offered) {
      failures.push(
        "the talk board's first message was not reviewed with Always allow this app",
      )
      return
    }
    await shot("review")
    await always.click()
    const firstSaid = await sent(30_000)
    if (firstSaid !== "ok")
      failures.push(`after Always allow the talk board says ${JSON.stringify(firstSaid)}`)
    if (!(await waitFor(async () => appTrusted(run, TEST_APP_SERVER), 5_000)))
      failures.push("Always allow this app was not kept in trusted-apps.json")
  }
  await expectShown(page, `${SAYS.echo}${firstText}`, failures)
  // Its next message goes as the person, with no review.
  const secondText = `${STRESS.reply} from the talk board r${step.index} second`
  await ask(secondText)
  const secondSaid = await sent(30_000)
  if (secondSaid !== "ok")
    failures.push(`the talk board's second message ended ${JSON.stringify(secondSaid)}`)
  if (await card.isVisible().catch(() => false))
    failures.push("the talk board's second message was reviewed after Always allow")
  const reviews = (await readWithReviews(run.client, conversationId)).permissions.filter(
    (review) => review.origin?.kind === "app",
  )
  if (reviews.length)
    failures.push(`${reviews.length} app reviews are still pending after Always allow`)
  await expectShown(page, `${SAYS.echo}${secondText}`, failures)
  // Its tab closed, nothing of it is left running.
  await tabs.last().click()
  const closed = await waitFor(
    async () => !(await page.$(selectorFor.appFrameIn("panel"))),
    5_000,
  )
  if (!closed)
    failures.push("the talk board's panel frame is still drawn after its tab closed")
  // The right panel stays open with its last tab gone; leave it as found,
  // or the next step that toggles it closes it instead.
  if (!panelWasShown && (await panelShown())) {
    await page.keyboard.press("Meta+Alt+KeyB")
    if (!(await waitFor(async () => !(await panelShown()), 3_000)))
      failures.push("⌥⌘B did not close the right panel after the talk board")
  }
  // Undone from Settings: the app leaves the list and the gateway's file, so
  // the next repetition finds it asked about again.
  await untrustFromSettings(run, TEST_APP_SERVER, failures, shot)
}

/**
 * Revokes "Always allow this app" where a person does: Settings › Privacy &
 * Permissions › Access, the app's row under Apps you always allow. Leaves
 * Settings, back in the workspace.
 */
async function untrustFromSettings(run, server, failures, shot) {
  const page = run.page
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await page.locator(css.settingsCategory, { hasText: names.privacy }).click()
  const tab = page.locator(css.settingsTab, { hasText: names.accessTab })
  if (await tab.count()) await tab.click()
  const row = page.locator(`[data-trusted-app="${server}"]`)
  const listed = await row
    .waitFor({ state: "visible", timeout: 10_000 })
    .then(() => true)
    .catch(() => false)
  if (!listed) {
    failures.push(`${server} is not listed under Apps you always allow`)
  } else {
    await shot("trusted-apps")
    await row.getByRole("button", { name: `Revoke ${server}` }).click()
    if (!(await waitFor(async () => (await row.count()) === 0, 10_000)))
      failures.push(`${server} is still listed after Revoke`)
    if (!(await waitFor(async () => !appTrusted(run, server), 5_000)))
      failures.push(`${server} is still in trusted-apps.json after Revoke`)
    const kept = await run.client.apps.trusted().catch(() => null)
    if (!kept || kept.includes(server))
      failures.push(`apps.trusted still answers ${JSON.stringify(kept)} after Revoke`)
  }
  await page.getByRole("button", { name: names.leaveSettings }).first().click()
  await page.waitForSelector(css.settings, { state: "detached", timeout: 10_000 })
}

/** What must hold once everything has run, in every conversation the run made. */
async function titleOf(run, conversationId) {
  const title = await waitFor(async () => {
    const { conversations } = await run.client.conversation.list({})
    return conversations.find((each) => each.conversationId === conversationId)?.title
  }, 15_000)
  if (!title)
    throw new CannotRun(`conversation ${conversationId} has no title to find it by`)
  return title
}

/** What the window has told the host about waiting reviews (`fake-host.mjs`). */
const hostNotices = (page) => page.evaluate(() => window.__fakeHostNotices ?? null)

const sameNotice = (a, b) =>
  a.conversationId === b.conversationId &&
  a.executionId === b.executionId &&
  a.permissionId === b.permissionId

/**
 * The notice the host shows for the review now waiting in `conversationId`,
 * and that review read in full; `null` (a failure noted) when none is up
 * within 30 s. `not` is a notice of the same conversation already settled.
 */
async function noticePosted(run, conversationId, failures, not = null) {
  let found = null
  await waitFor(async () => {
    const view = await readWithReviews(run.client, conversationId).catch(() => null)
    const review = view?.permissions[0]
    const notices = await hostNotices(run.page)
    const notice =
      review &&
      notices?.up.find(
        (each) =>
          each.conversationId === conversationId &&
          each.executionId === review.executionId &&
          each.permissionId === review.permissionId,
      )
    if (notice && !(not && sameNotice(notice, not))) found = { notice, review }
    return Boolean(found)
  }, 30_000)
  if (!found) failures.push(`no notice was posted for the review in ${conversationId}`)
  return found
}

/** Waits for the host to take `notice` back once its review was `how`. */
async function noticeTakenBack(run, notice, how, failures) {
  const gone = await waitFor(async () => {
    const notices = await hostNotices(run.page)
    return (
      notices !== null &&
      !notices.up.some((each) => sameNotice(each, notice)) &&
      notices.withdrawn.some((each) => sameNotice(each, notice))
    )
  }, 15_000)
  if (!gone) failures.push(`the notice of a review ${how} was not taken back`)
}

async function atRest(run, ids) {
  const failures = []
  for (const conversationId of ids) {
    const { view } = await untilView(
      run,
      conversationId,
      (current) => current.messages.every(ended) && current.pending.length === 0,
      60,
    )
    if (!view) {
      failures.push(`${conversationId} could not be read`)
      continue
    }
    const running = view.messages.filter((each) => !ended(each))
    if (running.length)
      failures.push(`${conversationId} still has ${running.length} turns at work`)
    for (const turn of view.messages) {
      const said = `${turn.error ?? ""} ${(turn.parts ?? []).map((part) => part.text ?? "").join(" ")}`
      if (/Denied automatically|No human answered/i.test(said))
        failures.push(`${conversationId}: a review was denied automatically`)
      if (/display limit/i.test(said))
        failures.push(`${conversationId}: a review was hidden`)
    }
    if (view.permissions.length) failures.push(`${conversationId} still has open reviews`)
  }
  // Every review settled, so the host shows none of their notices.
  const cleared = await waitFor(async () => {
    const notices = await hostNotices(run.page)
    return notices !== null && notices.syncs > 0 && notices.up.length === 0
  }, 15_000)
  if (!cleared) {
    const notices = await hostNotices(run.page)
    failures.push(
      !notices?.syncs
        ? "the window never told the host which reviews wait"
        : `${notices.up.length} review notices are still up`,
    )
  }
  const text = await pageText(run.page)
  if (/display limit|Denied automatically/i.test(text))
    failures.push("the window says a review was hidden or denied automatically")
  return failures
}

async function startStack(options) {
  const profile = stressProfile(options)
  const knobs = knobsFrom(options)
  const selected = selectedPlan(knobs, options.only, profile.scenario)
  if (selected.some((step) => step.kind === "inputFiles")) phoneFilesVerifier()
  if (selected.some((step) => step.kind === "pairedCatalogue"))
    await preparePairedCatalogue()
  // Linking listens on every interface, as Settings sets it up, so the
  // gateway announces itself; clients on this machine dial it on loopback.
  const nativePort = await freePort()
  const nativeAddress = `127.0.0.1:${nativePort}`
  const nativeListen = `0.0.0.0:${nativePort}`
  const directory = mkdtempSync(join(tmpdir(), "nessa-stress-"))
  const nativeContext =
    knobs.drifts > 0
      ? (() => {
          const paths = Object.fromEntries(
            ["directory", "workspace", "baseline"].map((key) => {
              const path = join(directory, `native-context-${key}`)
              mkdirSync(path)
              return [key, realpathSync(path)]
            }),
          )
          return { ...paths, channel: null }
        })()
      : undefined
  // Stop's dispatch assertion also runs without an exported evidence folder.
  const recordingDirectory = options.evidence ?? join(directory, "recording")
  const scenario = join(directory, "stress.json")
  writeFileSync(
    scenario,
    JSON.stringify(
      stressScenario({
        agent: profile.agent,
        reviews: knobs.reviews,
        longChunks: LONG_CHUNKS,
        longChunkBytes: LONG_CHUNK_BYTES,
        oversizedChunks: OVERSIZED_CHUNKS,
      }),
    ),
  )
  if (options.evidence) {
    mkdirSync(options.evidence, { recursive: true })
    writeFileSync(
      join(options.evidence, "plan.txt"),
      `${JSON.stringify({ ...knobs, profile })}\n${options.only ? `SELECTED SCOPE: ${options.only}\n` : "FULL PLAN\n"}${describe(selected)}\n`,
    )
  }
  // The `github` step's GitHub, on this machine, which the gateway reads its
  // app from, and Nessa's own shell tool, where an agent's git and gh run.
  let servers = []
  try {
    if (knobs.githubs > 0) servers = [shellServer(directory)]
    // A second app server, so the window step's chat can hold two panel tabs.
    if (selected.some((step) => step.kind === "window"))
      servers.push({
        name: "panelpin",
        command: process.execPath,
        args: [join(repoRoot, "scripts/mcp-test-server/panel-pin.mjs")],
      })
  } catch (error) {
    rmSync(directory, { recursive: true, force: true })
    throw error
  }
  // The `ssh` step's stand-in `ssh`, first on the gateway's PATH.
  const fakeSsh = selected.some((step) => step.kind === "ssh")
    ? writeFakeSsh(join(directory, "fake-ssh"))
    : null
  const github = knobs.githubs > 0 ? await startFakeGithub() : null
  // The `voice` step's realtime provider, on this machine.
  const realtime = knobs.voices > 0 ? await startFakeRealtime() : null
  // The `chatgpt` step's OpenAI sign-in, on this machine.
  const openai = knobs.chatgpts > 0 ? await startFakeOpenai() : null
  const environment = {
    ...(openai
      ? { NESSA_CHATGPT_SIGN_IN: openai.config(join(directory, "chatgpt-sign-in.json")) }
      : {}),
    ...(nativeContext ? { CLAUDE_CONFIG_DIR: nativeContext.baseline } : {}),
    ...(realtime
      ? {
          NESSA_VOICE_REALTIME: realtime.provider(join(directory, "voice-realtime.json")),
        }
      : {}),
    ...(github
      ? {
          ...disposableGithubEnvironment(),
          NESSA_GITHUB_APP: github.app(join(directory, "github-app.json")),
        }
      : {}),
    // The `tunnel` step's cloudflared (debug builds only).
    NESSA_TUNNEL_PROGRAM: fakeCloudflared(directory),
    ...(fakeSsh ? { PATH: `${fakeSsh.bin}:${process.env.PATH ?? ""}` } : {}),
  }
  let stack
  try {
    const vmFixture = selected.some((step) =>
      ["vmBackground", "cloud"].includes(step.kind),
    )
      ? await vmBackgroundFixture(directory, PARK_IDLE_SECONDS)
      : null
    Object.assign(environment, vmFixture?.environment)
    let restoreReport = null
    const refusals = join(directory, "refusals")
    mkdirSync(refusals, { recursive: true })
    stack = await startGatewayStack(options, INSTANCE, {
      executable: vmFixture?.executable,
      vault: true,
      beforeListen:
        knobs.restores > 0
          ? async ({ directory, stage, instance }) => {
              const bin = join(repoRoot, "target/debug/nessa-backup-restore")
              const destination = join(directory, "stress-restore")
              const namespace = join(directory, stage, "instances", instance)
              const result = spawnSync(
                bin,
                [
                  "prepare",
                  "--destination",
                  destination,
                  "--namespace",
                  namespace,
                  "--count",
                  String(knobs.restores),
                ],
                { encoding: "utf8" },
              )
              if (result.status !== 0) {
                throw new Error(
                  `stress restore prepare failed: ${result.stdout ?? ""}\n${result.stderr ?? ""}`,
                )
              }
              const line = result.stdout
                .trim()
                .split("\n")
                .filter((each) => each.includes('"prepared"'))
                .at(-1)
              restoreReport = JSON.parse(line)
            }
          : undefined,
      environment,
      servers,
      webhooks:
        knobs.webhooks > 0
          ? [
              {
                principalId: "stress-webhook",
                url: "https://localhost/synthetic",
                macKey: Buffer.alloc(32, 7).toString("base64"),
              },
            ]
          : undefined,
      as: "panel",
      scenario,
      nativeContext,
      model: profile.model,
      evidence: recordingDirectory,
      limits: { agentIdleSeconds: knobs.parks > 0 || vmFixture ? PARK_IDLE_SECONDS : 0 },
      // Where the `unsigned` and `failedControl` steps ask the scripted agents
      // to refuse a call (`scripted-agent.mjs` `--refusals`).
      refusals,
      // The agent a `handoff` step switches its conversation to.
      others: [
        ...new Set([
          ...(knobs.handoffs > 0 || knobs.unsigned > 0
            ? [otherAgent(options.agent)]
            : []),
          ...(nativeContext && options.agent !== "claude" ? ["claude"] : []),
          // The `chatgpt` step runs Codex on its worker.
          ...(knobs.chatgpts > 0 && options.agent !== "codex" ? ["codex"] : []),
        ]),
      ],
      // Off, at a port of its own: the `linking` step turns it on from
      // Settings. Its tunnel too, at a loopback port of its own.
      native: {
        enabled: false,
        listenAddress: nativeListen,
        tunnel: { enabled: false, ingress: `127.0.0.1:${await freePort()}` },
      },
    })
    const { agents } = await stack.client.agents.list()
    stack.profile = selectStressFixture(profile, agents)
    stack.vmFixture = vmFixture
    stack.refusals = refusals
    stack.fakeSsh = fakeSsh
    // The PATH the gateway was started with: a step that changes it adds to
    // this one, so what another step put there (the `ssh` step's stand-in)
    // stays for the launches after it.
    stack.gatewayPath = environment.PATH ?? process.env.PATH ?? ""
    stack.restore = restoreReport
  } catch (error) {
    await stack?.close()
    await github?.close()
    await realtime?.close()
    await openai?.close()
    rmSync(directory, { recursive: true, force: true })
    throw error
  }
  const close = stack.close
  stack.close = async () => {
    try {
      await close()
    } finally {
      await github?.close()
      await realtime?.close()
      await openai?.close()
      rmSync(directory, { recursive: true, force: true })
    }
  }
  return panelTarget(stack, () => ({
    knobs,
    profile: stack.profile,
    agent: options.agent,
    acpRecording: join(recordingDirectory, "acp.jsonl"),
    nativeAddress,
    nativeListen,
    github,
    realtime,
    openai,
    scenario,
    nativeContext,
    // Removed when the stack closes.
    scratch: directory,
    endpoint: stack.gateway.url.replace(/^http/, "ws"),
  }))
}

/** `text` as a pattern that matches only itself. */
function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
}

await main(
  meta,
  async ({ options, rep, target: stack }) => {
    const knobs = stack.knobs
    const planned = selectedPlan(knobs, options.only, stack.profile.scenario)
    log(
      `conversation-stress ${JSON.stringify({ ...knobs, profile: stack.profile })}\n${describe(planned)}`,
    )
    const origin = new URL(stack.url).origin
    await withEngines(options, rep, async (engine, browser) => {
      const layout = options.layouts[0]
      // Each engine opens a fresh simulated app. Re-establish its owned
      // disposable vault session after the prior engine's gateway restarts.
      await stack.vault.unlock()
      let restartConsole
      let reviewAnswers
      let accessRefusals
      const gateway = new URL(stack.gateway.url)
      // The packaged window's connect-src, plus the ports only this check
      // has: its own gateway's, and the fake voice provider the window's
      // scripted transport calls in place of WebRTC. A read the app's policy
      // blocks fails here too (`paste`, `attach`).
      const contentPolicy = appConnectPolicy([
        gateway.origin,
        `ws://${gateway.host}`,
        ...(stack.realtime ? [stack.realtime.socketOrigin] : []),
      ])
      const opened = await openPage(browser, {
        contentPolicy,
        preparePage: async (page) => {
          await stack.vault.install(page)
          restartConsole = observeGatewayRestarts(page, stack.endpoint)
          reviewAnswers = observeReviewAnswers(page, stack.endpoint)
          accessRefusals = observeAccessRefusals(page, stack.endpoint)
        },
        url: `${origin}/desktop.html`,
        layout,
        lines: { engine, layout },
        initScripts: [
          [
            gatewayHost,
            {
              endpoint: stack.endpoint,
              credential: stack.credential,
              notices: true,
              starting: STARTING,
            },
          ],
        ],
      })
      // The probe's own refusal is the console line it is expected to leave.
      opened.noteHarmless(/127\.0\.0\.1:9\/policy-probe/)
      await assertPolicyHolds(opened.page, contentPolicy)
      const run = new Run(stack, opened, knobs, restartConsole, waitFor)
      run.reviewAnswers = reviewAnswers
      run.reviewAnswerEvidence = []
      run.accessRefusals = accessRefusals
      run.accessRefusalEvidence = []
      run.evidence = options.evidence
      run.engine = engine
      const base = {
        engine,
        layout,
        scope: options.only ? `selected:${options.only}` : "full",
      }
      let followers = []
      try {
        const id = randomUUID()
        const first = `${STRESS.reply} main s${knobs.seed} ${engine}`
        const setup = []
        await run.client.conversation.create({ conversationId: id, agent: stack.agent })
        await run.client.conversation.submit(run.client.conversation.prepare(id, first))
        await settled(run, id, new Set(), first, "completed", setup)
        run.main = { id, title: await titleOf(run, id) }
        // A previous engine's restart can leave the shared gateway vault locked.
        await unlockRequestedVault(run.client, run.page)
        await openStressConversation(run.page, run.main.id, run.main.title)
        const initial = await readWithReviews(run.client, run.main.id)
        const verifiedProfile = requireStressCapabilities(stack.profile, {
          modelId: initial.runtime?.model,
          approvalModes: initial.approvalModes,
        })
        if (initial.approvalMode !== "ask")
          setup.push("the stress fixture did not start in Ask")
        if (run.evidence)
          writeView(run.evidence, `views/${engine}-profile.json`, verifiedProfile)
        if (setup.length) {
          rep.add({ name: "setup", ...base, failures: setup })
          return
        }
        run.background = Array.from({ length: knobs.parallel }, (_, n) =>
          background(run, n),
        )
        const { subscriptionLimits } = await import("@nessa/client")
        followers = Array.from(
          { length: knobs.followers },
          (_, n) =>
            new Follower({
              n,
              connect: () => connectClient(stack.gateway, INSTANCE, "panel"),
              limits: subscriptionLimits,
              random: random(knobs.seed * 1_000 + n + 1),
              mainId: run.main.id,
              deleted: run.deleted,
            }),
        )
        await Promise.all(followers.map((each) => each.start()))
        let stopped = null
        for (const step of planned) {
          const name = `${String(step.index).padStart(3, "0")} ${step.kind}`
          if (stopped) {
            rep.add({ name, ...base, cannotRun: true, error: `not run: ${stopped}` })
            continue
          }
          run.failureConversationId = null
          const entry = await reportStressStep(rep, { name, ...base }, step, (failures) =>
            steps[step.kind](run, step, failures),
          )
          if (!entry.ok) {
            stopped = `${name} did not hold`
            if (options.evidence) {
              const view = await readFailureView(run, readWithReviews).catch(() => null)
              if (view)
                writeView(
                  options.evidence,
                  `views/${engine}-${name.replace(" ", "-")}.json`,
                  view,
                )
              await run.page
                .screenshot({
                  path: join(options.evidence, `${engine}-${name.replace(" ", "-")}.png`),
                })
                .catch(() => {})
            }
          }
        }
        const done = await Promise.all(run.background)
        rep.add({
          name: "background",
          ...base,
          seen: { conversations: done.length, turns: knobs.turns },
          failures: done.flatMap((each) => each.failures),
        })
        rep.add({
          name: "at rest",
          ...base,
          failures: await atRest(run, [
            run.main.id,
            ...run.owned,
            ...run.checked,
            ...done.map((each) => each.conversationId),
          ]),
        })
        await Promise.all(followers.map((each) => each.settle()))
        const followed = []
        for (const each of followers) followed.push(...(await each.check(run.client)))
        rep.add({
          name: "followers",
          ...base,
          seen: followers.map((each) => ({
            conversations: each.events.size,
            ...each.counts,
          })),
          failures: followed,
        })
        if (options.evidence) {
          const view = await readWithReviews(run.client, run.main.id).catch(() => null)
          if (view) writeView(options.evidence, `views/${engine}-main.json`, view)
        }
      } finally {
        await Promise.allSettled(run.background)
        await Promise.allSettled(followers.map((each) => each.close()))
        stack.client = run.client
        try {
          await opened.close()
        } finally {
          restartConsole.dispose()
          reviewAnswers.dispose()
          accessRefusals.dispose()
        }
      }
    })
  },
  startStack,
)
