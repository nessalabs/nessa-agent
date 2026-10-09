# 689 — Linux service-definition replacement

Issues #689 and #690 own this correction to the systemd adapter. The adapter
validates a retained definition by recovering its Claude directory from its
canonical bytes and rendering all its other ownership inputs from the desktop.
This accepts a prior directory without accepting arbitrary service bytes.
Generation reuse requires equality with the complete desired command, so a
changed PATH, Claude directory, runtime or other command input selects a new
incarnation before effects are admitted.

| Retained state / event                                                               | Decision and effect order                                                                                                            | Regression                                                       |
| ------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------- |
| Exact running unit, unchanged command                                                | Reuse generation; readiness corroboration; no stop/reload/start                                                                      | unchanged definition reuse                                       |
| Exact running unit, new PATH or Claude directory                                     | Fresh generation; admit before/after targets; retire exact prior; stop; publish; reload; start; confirm ready                        | command replacement selection and retained-definition validation |
| Exact inactive/unloaded definition, changed directory                                | Validate actual old directory; publish desired definition; reload; start                                                             | inactive prior directory validation                              |
| Quoted environment value contains a Claude-looking key / repeated Claude assignments | Parse complete arguments; select one unique assignment or refuse duplicates                                                          | unique environment-argument recovery                             |
| Foreign bytes or directory                                                           | Refuse before replacement; retain bytes/process                                                                                      | canonical owned-render rejection                                 |
| Settings publication pending / another reconciliation reads live configuration       | Hold the directory-publication claim without changing live configuration; publish live only after durable acknowledgement            | unconfirmed settings publication rollback                        |
| Settings save returns failure after its update callback obtained the prior value     | Retain prior publication authority, restore the desired value if it landed, refuse native dispatch and preserve any rollback failure | unconfirmed settings publication rollback                        |
| Failure before replacing old definition                                              | Retain old definition; durable desired settings remain retry intent                                                                  | old definition remains valid against desired settings            |
| Failure after publication or desktop restart                                         | Recovery inspects canonical on-disk definition and journal before new effects; no replay of an uncertain command                     | existing recovered definition/effect tests                       |

Durable settings publication shares the existing `ClaudePublication` owner with
live configuration and reconciliation. The command injects its settings adapter.
The owner publishes durable intent before reconciliation and restores the prior
durable directory on failure or dropped caller before releasing publication
ownership. Identical requests join that outcome and different requests wait;
even an unchanged live directory holds publication ownership while saving.
A rollback failure is a typed compound result retaining the original failure.
Restart recovery still validates the actual installed canonical configuration,
since an interrupted native effect may have landed before failure was recorded.
