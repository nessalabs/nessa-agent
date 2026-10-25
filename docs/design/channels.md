# Channels

A channel is a named place a person's conversations live in. It holds what a
new conversation in it starts with, and where its agents work:

- **Defaults**: the agent, the model and the approval preset a new
  conversation starts with. Each is only a start; the person can change any of
  them inside the conversation.
- **Folder**: where every agent in the channel works. The person picks an
  existing folder, or the gateway makes `<data>/channels/<id>`
  (`~/.nessa/channels/<id>` on a desktop). Two channels may share a folder.
- **Notes and resources**: text, and absolute paths to files and folders, that
  every agent in the channel is told in its system instructions.

Every conversation is in exactly one channel for its whole life. A
conversation with no stored channel is in the **default channel**, whose folder
is the gateway's configured workspace. The default channel is never stored.

## A request names a channel, never a folder

`channel.create` is the only method that takes a path, and Cedar admits it
only for the owner (`owner.channel_create`). The gateway resolves the folder,
follows links, and refuses the root, the home folder itself, system folders and
its own data other than its `channels/` folder. After that, `conversation.create`
names a `channelId`, and Cedar checks `channel.use` on that channel. What a
request can select is therefore always a place its owner already made.

```mermaid
sequenceDiagram
    participant Desktop
    participant Gateway as Gateway (product/channel.rs)
    participant Service as ConversationService
    participant Cedar
    participant Folders as LocalChannelFolders
    participant Store as LocalChannelStore
    Desktop->>Gateway: channel.create {requestId, name, folder?, agent, model, approvalMode?, notes?, resources?}
    Gateway->>Service: create_channel(caller, requestId, ChannelRequest)
    Service->>Cedar: channel.create on the caller's catalogue
    Service->>Folders: accept_folder / accept_resource (resolved)
    Service->>Store: by_request(org, owner, requestId)
    alt no folder chosen
        Service->>Folders: make(ch_…) under <data>/channels
    end
    Service->>Store: create(channel)
    Gateway-->>Desktop: {channel}
    Desktop->>Gateway: conversation.create {channelId}
    Service->>Cedar: channel.use on the channel
    Service->>Store: place(conversation, channel)
    Service->>Service: resolve_in(agent, model, mode, AgentPlace{folder, instructions})
```

## Ids and retries

A channel's id is `ch_` and ten base32 characters of
`sha256("nessa.channel.create\0", org, owner, requestId)`. A retried
`channel.create` names the same id, and so the same made folder. The kept
channel is answered when the retry asks for the same channel, and
`channel_request_conflict` otherwise.

## Where an agent works

The service looks up a conversation's channel each time it builds an agent
(creation, opening, model and mode changes, handoffs) and hands the source an
`AgentPlace`. `CurrentAgentResolver::resolve_in` builds the agent with its
workspace set to the channel folder and the channel's instructions appended to
Nessa's own. Nothing built for the configured workspace is reused, including
OpenCode's warm-up. A provider session's erasure is asked from the same place
(`ProviderSessionEraser::erase_in`), and the placement is let go only once the
erasure is settled.

A thread an agent starts with `create_thread` is in its starting conversation's
channel, unless it runs on another host. A channel conversation always runs on
this machine: a channel with an `environment` is refused.

## Making one in the desktop window

The sidebar's + opens the "New channel" flow over the session list and the
panes (`src/desktop/workspace/ui/channel-creation/`). It asks one question at
a time, the way an agent would, and folds each answer into a line that asks it
again when clicked: the name, where its agents work, the model new
conversations start on, notes, and files or folders to attach. Only the name
must be typed; every other question starts with an answer (a folder made for
the channel, the window's default model, no notes, nothing attached), so
Create is ready from the first answer.

```mermaid
sequenceDiagram
    participant Person
    participant Flow as New channel flow
    participant Host as Desktop host
    participant Gateway
    Person->>Flow: + beside the section
    Flow->>Host: home_folder (once)
    Person->>Flow: name, then Return
    Person->>Flow: Choose a folder…
    Flow->>Host: choose_channel_folder
    Host-->>Flow: /Users/me/site (or nothing: cancelled)
    Person->>Flow: model, notes, Add files…
    Flow->>Host: choose_channel_resources {folders}
    Person->>Flow: Create (⌘↩)
    Flow->>Gateway: channel.create {requestId, name, folder?, agent, model, approvalMode, notes?, resources?}
    alt refused
        Gateway-->>Flow: channel_folder_refused
        Flow-->>Person: said at the folder question, asked again
    else made
        Gateway-->>Flow: {channel}
        Flow-->>Person: the channel opens on a new session's home
        Person->>Gateway: conversation.create {channelId} with the first message
    end
```

The flow's state is the store's (`usecases/channel-creation.ts`), so an agent
drives it with the same commands a person's clicks dispatch. A Create keeps
its request id while the draft stays as sent, so asking again after no answer
makes one channel; any change to the draft takes a fresh id.

The paths come only from the system's pickers: the host's
`choose_channel_folder` and `choose_channel_resources` commands
(`src-tauri/src/channel_paths.rs`), composed in `main.tsx` on macOS and
Windows. Linux has no picker of its own yet, and a browser none at all, so
there the flow does not ask for a folder or files: the channel works in a
folder made for it.

## What comes later

Settings that bound a conversation instead of starting it (inbound and output
policies, cost thresholds) are kept beside the defaults, each checked through
the Cedar policy port. Adding one does not change how a conversation names its
channel. Renaming, moving and deleting channels are not built yet.

## Known gaps

- OpenCode has no system prompt, so it works in the channel folder but is not
  told the notes or resources.
- Fixed (non-managed) agent bindings erase sessions as from the configured
  workspace.
- A channel made by another client with an approval preset other than Ask
  starts its new sessions there, but the desktop composer shows Ask until the
  conversation says otherwise. The desktop flow does not ask for a preset.
- Dropping files on the flow does not attach them yet; the pickers do.
