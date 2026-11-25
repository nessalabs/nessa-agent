---- MODULE SamePerson ----
VARIABLES phase, samePerson, owner, issued, linked, linkAudit, endCause, removalReceipt
vars == <<phase, samePerson, owner, issued, linked, linkAudit, endCause, removalReceipt>>

Causes == {"CredentialRevoked", "EnrollmentRevoked", "Expired"}

Init ==
    /\ phase = "BeforeChoice"
    /\ samePerson = FALSE
    /\ owner = "None"
    /\ issued = FALSE
    /\ linked = FALSE
    /\ linkAudit = FALSE
    /\ endCause = "None"
    /\ removalReceipt = "None"

SaveChoice(choice, principal) ==
    /\ phase = "BeforeChoice"
    /\ ~choice \/ principal = "Human"
    /\ phase' = "ChoiceSaved"
    /\ samePerson' = choice
    /\ owner' = principal
    /\ UNCHANGED <<issued, linked, linkAudit, endCause, removalReceipt>>

Activate ==
    /\ phase = "ChoiceSaved"
    /\ phase' = IF samePerson THEN "LinkOwed" ELSE "NoLink"
    /\ issued' = TRUE
    /\ UNCHANGED <<samePerson, owner, linked, linkAudit, endCause, removalReceipt>>

SaveLink ==
    /\ phase' = "Linked"
    /\ linked' = TRUE
    /\ linkAudit' = TRUE
    /\ endCause' = "None"
    /\ UNCHANGED <<samePerson, owner, issued, removalReceipt>>

CommitLink ==
    /\ phase = "LinkOwed"
    /\ SaveLink

EndCredential(cause) ==
    /\ phase \in {"LinkOwed", "Linked"}
    /\ phase' = "RemovalOwed"
    /\ endCause' = cause
    /\ UNCHANGED <<samePerson, owner, issued, linked, linkAudit, removalReceipt>>

CommitRemoval ==
    /\ phase = "RemovalOwed"
    /\ phase' = "Removed"
    /\ linked' = FALSE
    /\ removalReceipt' = endCause
    /\ UNCHANGED <<samePerson, owner, issued, linkAudit, endCause>>

LinkAfterClockRollback ==
    /\ phase \in {"RemovalOwed", "Removed"}
    /\ endCause = "Expired"
    /\ ~linked
    /\ removalReceipt = "None"
    /\ SaveLink

Wait ==
    /\ phase \in {"LinkOwed", "RemovalOwed", "NoLink", "Removed"}
    /\ UNCHANGED vars

Next ==
    \/ \E choice \in BOOLEAN, principal \in {"Human", "Integration"} : SaveChoice(choice, principal)
    \/ Activate \/ CommitLink
    \/ \E cause \in Causes : EndCredential(cause)
    \/ CommitRemoval \/ LinkAfterClockRollback \/ Wait

Read(granted, bindingMatches) ==
    [allowed |-> granted, person |-> granted /\ linked /\ bindingMatches]

HumanChoiceOnly == samePerson => owner = "Human"
LinkNeedsActivationAndAudit ==
    /\ linked => (samePerson /\ issued /\ phase \in {"Linked", "RemovalOwed"} /\ linkAudit)
    /\ phase = "Linked" => linked
RemovalKeepsCause == phase = "Removed" => (endCause \in Causes /\ removalReceipt = endCause)
RetiredChoiceStaysConsumed == removalReceipt # "None" => (phase = "Removed" /\ ~linked)
AttributionNeverGrantsRead == \A matches \in BOOLEAN : ~Read(FALSE, matches).allowed
AttributionMatchesBinding ==
    \A granted, matches \in BOOLEAN : Read(granted, matches).person => (granted /\ linked /\ matches)
====
