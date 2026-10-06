# Operational limits

Every limit the gateway can refuse on is named in `docs/limits.md`, which
`crates/nessa-server/src/limits.rs` renders from the owners. The registry
states no number. A refusal that hits a limit logs that id; the wire response
and the close code stay what they were. Putting the id on the wire is a
later schema change and is not this catalogue.

Three tiers:

| tier | where the number lives | examples |
| --- | --- | --- |
| fixed | the owner that already enforces it: the product schema, or a socket constant | request frame, record slot, record lane, refusal lane, discovery steps |
| configured | `OperationalLimits`, from `config.json` `limits` when the section is present | admission counts, app calls per socket, cold-read budget |
| derived | arithmetic on a configured value, not a second stored number | app calls per mount (one less than the lane), ordinary lane (ordinary slots plus app calls), control lane (control slots) |

The record slot, the record lane, and the refusal lane stay fixed. Install
admission stays where it is and is not a field of `limits`.

`socket.write_timeout` is the session section's write deadline, already
configurable. Both delivery deadlines are `RECORD_SEND_TIMEOUT`.

## What a configuration file may say

`limits` is camelCase, refuses an unknown field, and fills anything it omits
from `OperationalLimits::default`. Counts are 1 to 65536. App calls per
socket are 2 to 65536, so one mount can leave a slot. The cold-read budget
is a positive duration an `Instant` can add. The same rules refuse a file
and an embedding caller.

| file | result |
| --- | --- |
| section absent, or the file absent | the defaults |
| one field set | that field changes; the rest stay the defaults |
| a count of zero, or above 65536 | startup refuses the file |
| app calls per socket of 1 | startup refuses the file |
| an unknown field | startup refuses the file |
| a budget of zero, or one an `Instant` cannot add | startup refuses the file |

`nessa limits` and `nessa limits --json` print those effective server values
as JSON on stdout. An unusable file prints nothing and fails as
`RuntimeConfig`. The client's preparing count is not in that JSON.
