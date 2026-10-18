# Gateway scale

How many conversations one gateway holds at once, what each one costs, and
what stops the next tenfold. The check is `pnpm test:scale`
(`scripts/scale/gateway-scale.mjs`); it runs nightly in
`.github/workflows/gateway-scale.yml` and by hand at any numbers.

## The check

It needs no desktop window and no model. A release gateway starts in a
temporary `ci` namespace, with Claude's runtime pointed at the SDK's
`scale_agent` example. That is a stdio ACP agent of a few hundred kilobytes
that answers every prompt with `Noted.`. The scripted Node agent is tens of
megabytes each, and ten thousand of them would measure Node rather than the
gateway. Clients are `@nessa/client` sockets spread over a few Node processes
(`client-worker.mjs`). The run:

1. opens the sockets;
2. creates the conversations, spread evenly over the sockets;
3. sends every conversation a message and waits for its `turn.ended`, once per
   round;
4. lets every conversation sit idle for a while;
5. waits for the gateway to park every idle agent (`--idle-seconds`, written
   as `limits.agentIdleSeconds`; [agent-parking.md](agent-parking.md)),
   holds with no agent running, then sends every conversation one more
   message, which starts its agent again and resumes its session;
6. lists conversations from every socket, walking `observe` when the list is
   cut.

It samples the gateway and every process under it once a second (memory,
threads, open files, CPU, child processes and the data directory) and the
machine (busy CPU and used memory). Every phase reports its resources: cores
used by the gateway, by its agents and by the whole machine, the gateway's
and the agents' resident memory, and the most and fewest agents running. It
prints one JSON verdict. The verdict fails on any failed operation, and when
an agent is still running after the park phase. Each turn also reports how long it
waited on its subscription, its send and the turn itself, so a slow run says
where the time went.

```sh
cargo build --release -p nessa-server
cargo build --release -p nessa-sdk --example scale_agent
ulimit -n "$(ulimit -Hn)"
pnpm test:scale -- --conversations 4000 --sockets 1000 --rounds 2 --evidence /tmp/scale
```

`--help` lists every knob; each is also `NESSA_SCALE_<NAME>`.

## What a conversation costs

Measured on 4 cores and 15 GB, with 4,000 conversations on 1,000 sockets and
two rounds of turns:

| | |
| --- | --- |
| creates | 4,000 of 4,000, 68 s, about 17 ms of gateway CPU each |
| turns | 8,000 of 8,000, about 26 a second, about 70 ms of gateway CPU each |
| gateway memory | about 1 GB, about 0.2 MB a conversation past the first 220 MB |
| open files | about 3 a conversation (the agent's pipes), plus 1 a socket |
| agents | one process each, about 2.3 MB resident |
| idle | under 0.05 of a core for 4,000 idle conversations |

So 10,000 conversations need about 31,000 open files, about 3 GB for the
gateway, and room for 10,000 small agent processes. The gateway raises its
own soft open-file limit to the hard one at start (`core/open_files.rs`). A
hard limit under that, or a `pid_max` under the agent count, is the machine's
limit, not the gateway's.

## What parking saves

Measured on the same machine with 1,000 conversations on 250 sockets, two
rounds, `--idle-seconds 30`:

| | agents running | gateway | agents' memory |
| --- | --- | --- | --- |
| idle, agents up (hold) | 1,000 | 0.13 of a core | about 2.3 GB |
| parked (parked hold) | 0 | 0.01 of a core | none |

Every agent was parked within 22 s of the park phase starting, and all 1,000
turns after it completed, each starting its agent again on its session
(median 14 s at 64 sends at a time, against 8.5 s with agents running; the
difference is 1,000 agent starts competing for 4 cores). The scale agent is
about 2.3 MB; a real Claude agent idles at about 260 MB and a Codex agent at
165 to 200 MB ([#998](https://github.com/nessalabs/nessa-agent/issues/998)), so
what parking frees on a real gateway is about a hundred times the table's
memory column.

## What was fixed to get here

- **Reads waited on each other.** All conversations' records share one
  event-stream SQLite runtime with one worker thread. The store admitted one
  read at a time, so the thread sat idle between reads while thousands of
  subscriptions waited, and at 1,000 conversations most turns failed their
  first read. The store now queues up to 64 reads at its worker
  (event-stream `MAX_QUEUED_READS`), the SDK runs up to 32 read steps at once
  (`record_lifecycle::MAX_READS`), and the runtime admits waits of a minute
  rather than five seconds (`record::scaled_config`).
- **A reader read more than it needed.** A caught-up subscription rechecked
  its stream's identity on every step, and its emitter read one frame at a
  time after the certificate pass had just read the page. It now reuses that
  page, and only checks identity when it has caught up. A fact save no longer
  rereads what it just appended.
- **Every metadata query was parsed again.** The conversation catalogue
  resolves several statements per row; they are now prepared once and cached
  (`store/cached.rs`). A list of 1,000 rows from 250 sockets went from 102 s
  to 34 s.
- **A long list closed the socket.** A list past one frame now cuts to the
  frame and says it is incomplete, so the caller walks `observe`.
- **Configured limits sized for a few sockets.** Request and control slots
  default to 16,384, record reads to 1,024, and the live conversation count
  is configurable (`limits.liveConversations`, 65,536 by default).
- **Agent stderr kept a task and a pipe per agent.** It is discarded.
- **Creates queued behind each other.** A create, reopen or delete now waits
  only on the same conversation.
- **Shutdown stopped every agent at once.** Each stop saves its end through
  the one store, so 4,000 at once all ran out of their 10 s budget. At most
  64 now start at a time, each budget starts with its own stop, and a stop
  still running after a second (waiting on its agent, not the store) gives
  its place to the next, so agents that will not stop hold no one else back.

## What stops 100,000

Tracked in the 100K issue, with the measurements. In short: one agent
process per open conversation that is never parked when idle, one SQLite
thread for every conversation's records, one audit writer that syncs a file
per record, one locked connection for metadata, and fixed 65,536 caps on
admission counts.
