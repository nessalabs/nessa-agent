---------------- MODULE HistoryRestore ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, flushed, tailClosed, agreement, providerMatch, recordOnly
vars == <<phase, flushed, tailClosed, agreement, providerMatch, recordOnly>>
Init == phase = "FlushRetained" /\ flushed = FALSE /\ tailClosed = FALSE /\ agreement = FALSE /\ providerMatch = FALSE /\ recordOnly = FALSE
Flush == phase = "FlushRetained" /\ phase' = "ReadTail" /\ flushed' = TRUE /\ UNCHANGED <<tailClosed, agreement, providerMatch, recordOnly>>
CompleteTail == phase = "ReadTail" /\ phase' = "Validate" /\ tailClosed' = TRUE /\ UNCHANGED <<flushed, agreement, providerMatch, recordOnly>>
IncompleteTail == phase = "ReadTail" /\ phase' = "AbortTail" /\ UNCHANGED <<flushed, tailClosed, agreement, providerMatch, recordOnly>>
AckAbort == phase = "AbortTail" /\ phase' = "Validate" /\ tailClosed' = TRUE /\ UNCHANGED <<flushed, agreement, providerMatch, recordOnly>>
Validate == phase = "Validate" /\ tailClosed /\ phase' = "Prepared" /\ agreement' = TRUE /\ UNCHANGED <<flushed, tailClosed, providerMatch, recordOnly>>
RecordOnly == phase = "Prepared" /\ phase' = "RecordOnly" /\ recordOnly' = TRUE /\ UNCHANGED <<flushed, tailClosed, agreement, providerMatch>>
MatchProvider == phase = "Prepared" /\ providerMatch' = TRUE /\ UNCHANGED <<phase, flushed, tailClosed, agreement, recordOnly>>
Resume == phase = "Prepared" /\ providerMatch /\ agreement /\ phase' = "Resume" /\ UNCHANGED <<flushed, tailClosed, agreement, providerMatch, recordOnly>>
Refuse == phase \in {"FlushRetained", "ReadTail", "AbortTail", "Validate", "Prepared"} /\ phase' = "Refused" /\ UNCHANGED <<flushed, tailClosed, agreement, providerMatch, recordOnly>>
Next == Flush \/ CompleteTail \/ IncompleteTail \/ AckAbort \/ Validate \/ RecordOnly \/ MatchProvider \/ Resume \/ Refuse
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"FlushRetained", "ReadTail", "AbortTail", "Validate", "Prepared", "RecordOnly", "Resume", "Refused"} /\ flushed \in BOOLEAN /\ tailClosed \in BOOLEAN /\ agreement \in BOOLEAN /\ providerMatch \in BOOLEAN /\ recordOnly \in BOOLEAN
PreparedNeedsCommittedTail == phase \in {"Prepared", "RecordOnly", "Resume"} => flushed /\ tailClosed /\ agreement
ResumeExactProvider == phase = "Resume" => providerMatch /\ ~recordOnly
=============================================================================
