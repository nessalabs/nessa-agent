---- MODULE SurfaceCall ----
EXTENDS Naturals

VARIABLES phase, attempt, call, socket, retired, sent, decision, matched, outcome
vars == <<phase, attempt, call, socket, retired, sent, decision, matched, outcome>>

Inputs == [inCatalog : BOOLEAN, vaultChange : BOOLEAN, cedarAllows : BOOLEAN]
Terminal == {"Refused", "Answered", "Lost"}

Init ==
    /\ phase = "Checking"
    /\ attempt = 1
    /\ call \in Inputs
    /\ socket = 1
    /\ retired = {}
    /\ sent = FALSE
    /\ decision = "Pending"
    /\ matched = FALSE
    /\ outcome = "Pending"

RejectLocally ==
    /\ phase = "Checking"
    /\ ~call.inCatalog
    /\ phase' = "Refused"
    /\ outcome' = "CatalogError"
    /\ UNCHANGED <<attempt, call, socket, retired, sent, decision, matched>>

Reauthenticate ==
    /\ phase = "Checking"
    /\ call.inCatalog
    /\ socket \in retired
    /\ socket < 2
    /\ socket' = socket + 1
    /\ UNCHANGED <<phase, attempt, call, retired, sent, decision, matched, outcome>>

Dispatch ==
    /\ phase = "Checking"
    /\ call.inCatalog
    /\ socket \notin retired
    /\ phase' = "Sent"
    /\ sent' = TRUE
    /\ UNCHANGED <<attempt, call, socket, retired, decision, matched, outcome>>

Decide ==
    /\ phase = "Sent"
    /\ decision = "Pending"
    /\ decision' = IF call.cedarAllows THEN "Allowed" ELSE "Denied"
    /\ UNCHANGED <<phase, attempt, call, socket, retired, sent, matched, outcome>>

Reply(idMatches) ==
    /\ phase = "Sent"
    /\ decision # "Pending"
    /\ idMatches
    /\ matched' = idMatches
    /\ phase' = IF decision = "Allowed" THEN "Answered" ELSE "Refused"
    /\ outcome' = IF decision = "Allowed" THEN "Payload" ELSE "GatewayCode"
    /\ UNCHANGED <<attempt, call, socket, retired, sent, decision>>

LoseExchange ==
    /\ phase = "Sent"
    /\ phase' = "Lost"
    /\ retired' = retired \cup {socket}
    /\ outcome' = IF call.vaultChange THEN "VaultUnconfirmed" ELSE "TransportError"
    /\ UNCHANGED <<attempt, call, socket, sent, decision, matched>>

NextCall ==
    /\ phase \in Terminal
    /\ attempt = 1
    /\ phase' = "Checking"
    /\ attempt' = 2
    /\ call' \in Inputs
    /\ sent' = FALSE
    /\ decision' = "Pending"
    /\ matched' = FALSE
    /\ outcome' = "Pending"
    /\ UNCHANGED <<socket, retired>>

Done == /\ phase \in Terminal /\ attempt = 2 /\ UNCHANGED vars

Next ==
    \/ RejectLocally \/ Reauthenticate \/ Dispatch \/ Decide
    \/ \E idMatches \in BOOLEAN : Reply(idMatches)
    \/ LoseExchange \/ NextCall \/ Done

CatalogBeforeSend == sent => call.inCatalog
CedarBeforePayload == outcome = "Payload" => call.cedarAllows
ReplyMustMatch == outcome \in {"Payload", "GatewayCode"} => matched
LossRetiresSocket ==
    /\ phase = "Lost" => socket \in retired
    /\ phase = "Sent" => socket \notin retired
LostVaultIsUnconfirmed == (phase = "Lost" /\ call.vaultChange) => outcome = "VaultUnconfirmed"
====
