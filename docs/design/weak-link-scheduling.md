# Weak-link and metered sync scheduling

Issue [#262](https://github.com/nessalabs/nessa-agent/issues/262). The live read
on a device is `conversation.subscribe` plus a checkpoint copy. The watch
scheduler the unpushed `codex/262-*` branches targeted was deleted with that
path (#257). This note schedules the path that exists. It does not change
`follow`'s ending rows: a subscription end still ends that loop.

## Who decides

`LinkSchedule` in `read_only_sync/domain/link.rs` is the only priority, quote,
pace and reconnect decision. `run_quantum` in `read_only_sync/application/link.rs`
calls one quantum only after `Admit::Go`. One caller submits the whole
candidate list. `follow`, the records driver and `held_file` stay what they
are: one quantum, once the schedule has admitted it.

| Existing owner | What it still decides | What it does not decide |
| --- | --- | --- |
| `follow` | When one subscription ends (its rows W10–W11) and how many times `source_preparing` retries inside that pass | Which of transcript, catalogue, history and artifact runs next |
| records `driver` | How many pages one admitted pass applies | Whether that pass may start |
| `held_file` | Retrying the same range after `temporarily_unavailable` | Whether the next range may start, and whether it outranks a transcript |

`source_preparing` retries and a busy artifact range stay inside the quantum
already admitted. They are not new admits, so they do not re-check the
allowance. The bytes the cache kept at `finish` or `cancel` are charged once.

`nessa-sync` still owns the pass, authorization and semantic progress. The
private cache still owns downloaded and applied positions. The host does not
read a clock. The caller applies a returned wait, then sends `pace_elapsed`
or `backoff_elapsed`.

```text
caller --> LinkSchedule.admit(all candidates) --> Admit | Defer | InFlight
                |                                  |
                |                               no bytes move
                v
         one quantum (one follow pass, one catalogue
         page budget, one history pass, or one artifact range)
                |
                v
         finish(kept) or cancel(kept) --> allowance reconciled, pace wait
```

Arrows are calls. `kept` is the payload bytes the cache kept. A read that
kept nothing, including a lost reply, is `cancel(0)`.

## Byte identity

Quote, reserve, charge and pace all count payload bytes: the bytes a page or
a range is allowed to hold.

| Interest | Quote, known before the read | Owner of the ceiling |
| --- | --- | --- |
| Open transcript or older history | the page `Limits` the driver will use, `max_payload_bytes` | the sync engine's page limit, passed in |
| Catalogue | `MAX_CATALOGUE_PAYLOAD_BYTES` | the catalogue page owner |
| Artifact range | `HELD_FILE_RANGE_BYTES` (32,768) | `domain/held_file.rs` |

`Unknown` means the caller has not classified the interest, so no ceiling
applies. A metered admit with `Unknown`, or with a quote above the remaining
allowance, defers before any read and does not start a cheaper candidate.
An unmetered link does not need a quote.

`kept` is payload bytes the cache kept, not a JSON body, not TLS, and not the
relay preamble. The socket's encoded-response ceiling
(`product.max_record_response_bytes`) stays that socket's refusal. This schedule
does not repeat it. A runner that reports `kept` above the quote is charged
the bytes it reported: allowance becomes `before − kept`, and `saturating_sub`
leaves zero when `kept` exceeds `before`. Production pages are refused by their
own owners before a page can exceed its quote; the hostile-runner test still
charges the reported `kept`.

`finish` and `cancel` use one reconciliation. Allowance before the admit minus
`kept`, saturating at zero. A zero-byte cancel restores the whole reserve.

Relay `Connect` and `Joined` are measurement-only. They do not change who is
admitted and they are not `kept`.

## State

`Idle` has no quantum. `InFlight` has exactly one. `Reconnecting` is idle with
a backoff the caller has not said elapsed. A loss reported during `InFlight`
is stored and applied at `finish` or `cancel`, which then lands in
`Reconnecting` rather than `Idle`. Sleep or unreachable reported during
`InFlight` is stored the same way: the quantum already admitted runs to the
bytes it kept, and the next admit sees the new fact. Bytes already in flight
are not unsent.

Deferral order when more than one fact holds, first match: `Unreachable`,
`Reconnect`, `Sleep`, `Backpressure`, then the quote. Unreachable is reported
even if the phone is also asleep.

| State | Event | Next | What moves |
| --- | --- | --- | --- |
| Idle, source Unreachable | any candidates | Idle | `Defer(Unreachable)`. No read and no write. Not an empty catalogue and not `complete` |
| Reconnecting | any candidates | Reconnecting | `Defer(Reconnect)` |
| Reconnecting | `backoff_elapsed` | Idle | nothing is copied |
| Idle, presence Sleep | any candidates | Idle | `Defer(Sleep)` |
| Idle, pace closed | any candidates | Idle | `Defer(Backpressure)` |
| Idle, pace closed | `pace_elapsed` | Idle, pace open | nothing is copied |
| Idle, metered, winner quote unknown or above the allowance | candidates | Idle | `Defer(UnknownQuote)` or `Defer(Allowance)`. No cheaper candidate starts |
| Idle, unmetered or quoted | candidates | InFlight | the winner below. Allowance reserved when metered |
| InFlight | `admit` | InFlight | `InFlight`. The original quantum keeps running |
| InFlight | `connection_lost`, `sleep`, or `unreachable` | InFlight, fact stored | nothing new starts |
| InFlight | `finish(kept)` or `cancel(kept)`, no stored loss | Idle | allowance becomes `before − kept`. If `kept > 0` and the pace is limited, the pace closes and the wait is `ceil(kept / rate)` seconds |
| InFlight | `finish(kept)` or `cancel(kept)`, loss stored | Reconnecting | same charge, then backoff |
| Idle | `finish` or `cancel` | Idle | no charge |
| Idle | `connection_lost` | Reconnecting | nothing is copied |
| any | `reachable` | source reachable; in flight stays in flight | nothing is copied. Reconnect and sleep are unchanged |
| Idle, metered | allowance set to a new remainder | Idle | nothing is copied. Refused while a quantum is in flight |

`pace_elapsed` and `backoff_elapsed` are caller events. The host stores a
closed flag, not a timestamp. `finish` of `kept = 0` does not close the pace.

Winner, first match, and it blocks anything under it:

| Subscription | Order |
| --- | --- |
| Up | open transcript, catalogue, other older history in candidate order, artifact range |
| Down | older history of the open conversation, catalogue, other older history, artifact range |

Subscription down is a fact the caller sets after `follow` has already ended.
It does not keep that loop running. An artifact runs only when no transcript,
catalogue or history candidate is in the list. A range already in flight
finishes before a transcript can replace it.

Foreground and background share this order. Sleep admits nothing.

## Invariants

Each line names the test that fails when the line is removed.

1. From idle, unreachable, reconnect, sleep and a closed pace copy nothing,
   in that report order, and a closed pace outranks the quote.
   `unreachable_sleep_reconnect_and_backpressure_copy_nothing` checks that
   each of those facts copies nothing.
   `deferral_order_is_unreachable_then_reconnect_then_sleep_then_pace` checks
   the order, including sleep ahead of a closed pace and a closed pace ahead
   of an unknown quote.
2. Metered `Unknown` or an over-allowance quote copies nothing and does not
   start the next candidate. `metered_unknown_quote_blocks_a_cheaper_candidate`.
3. `finish` and `cancel` both leave `before − kept`, saturating at zero.
   Cancel of zero restores the reserve. `finish_and_cancel_leave_the_allowance_minus_what_was_kept`.
4. A second admit while in flight is `InFlight` and does not start work.
   `a_second_interest_is_in_flight_until_settle`.
5. The winner order above, including subscription-down fallback.
   `open_transcript_outranks_catalogue_history_and_artifact`.
6. A loss during the quantum charges `kept` and the next admit is
   `Defer(Reconnect)` until `backoff_elapsed`.
   `loss_during_the_quantum_reconnects_after_cancel`.
7. `finish` or `cancel` from idle does not charge.
   `finish_from_idle_does_not_charge`.
8. Route does not change the winner. `relay_does_not_change_who_is_admitted`.
9. The pace wait is `ceil(kept / rate)` seconds, the pace stays closed until
   `pace_elapsed`, and an admit before that copies nothing.
   `pace_stays_closed_until_the_caller_says_so`.

## Alternatives that lose

| Alternative | Why it loses |
| --- | --- |
| Revive the unpushed watch scheduler | #257 deleted that path. Nothing calls it |
| Put quotes in `nessa-sync` | Timers and link policy stay out of the core. The core owns the pass |
| Long polling beside `conversation.subscribe` | No measurement here shows a second live path is needed. `follow` keeps its ending rows |
| Let a missing transcript quote fall through to an artifact | The allowance would be spent on the cheaper interest |
| Skip reconnect when the loss arrives during the quantum | That is the loss that happens while bytes are moving |
| Treat pace as a duration the host measures | The host would read a clock. The caller sends `pace_elapsed` |
| Close fragment reassembly from this note | Not measured. Whole pages stay because that is the page contract already shipped, not because a fragment comparison was run. The comparison remains open |

## Measurements

The harness is `weak_link_measure`. It writes one real SDK record source (the
store a gateway reads; this process does not open a second gateway), copies
it through `run_records` one page at a time into one real private cache,
unpaced and paced at 8,000 payload bytes/s, then reopens the cache. Both
profiles keep the same payload bytes and the same downloaded and applied
positions. Applied lag on the paced profile is the schedule wait above, on a
caller-owned virtual clock. Wall time is recorded as what that process took
and is not a percentile. The numbers from one run, with `wallMs` left as
whatever that process took, are `docs/design/weak-link-measurements.json`.
The test `paced_and_unpaced_keep_the_same_bytes_and_reopen` re-runs the
harness and requires every field except `wallMs` to match that file.

The size table publishes `ceil(n / rate)` for 1, 4, 8, 16, 32 and 64 KiB at
2,000, 8,000 and 32,000 payload bytes/s (16, 64 and 256 kbit/s of this same
payload unit, not of a TLS byte). p50 and p95 in the report are order
statistics of that table, labeled as such.

Foreground, background, reconnect, sleep and metered rows are the state table.
Rows that defer move zero payload bytes. The relay row adds the encoded
reader `Connect` and `Joined` frames to a protocol column and does not call
them payload.

A phone radio, an OS sleep, TLS, IP billing and a sampled production
percentile are not in the harness. The device session does not call
`LinkSchedule` yet: presence, allowance and pace are phone facts it does not
have. That call is [#1062](https://github.com/nessalabs/nessa-agent/issues/1062).
