---- MODULE NativeLinkHandoff ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES operation, kind, stage, opener, navigate, accepted, failed, notice, live, published, beforeNotice
vars == <<operation, kind, stage, opener, navigate, accepted, failed, notice, live, published, beforeNotice>>
Init == /\ operation = 1 /\ kind \in {"own","web","unsupported"} /\ stage = "ProposedNavigation" /\ opener = FALSE /\ navigate = FALSE /\ accepted = {} /\ failed = {} /\ notice = 0 /\ live = TRUE /\ published = FALSE /\ beforeNotice = 0
ClassifyOwn == /\ stage = "ProposedNavigation" /\ kind = "own" /\ stage' = "InApp" /\ navigate' = TRUE /\ UNCHANGED <<operation,kind,opener,accepted,failed,notice,live,published,beforeNotice>>
ClassifyWeb == /\ stage = "ProposedNavigation" /\ kind = "web" /\ stage' = "ExternalHandoff" /\ opener' = TRUE /\ UNCHANGED <<operation,kind,navigate,accepted,failed,notice,live,published,beforeNotice>>
Refuse == /\ stage = "ProposedNavigation" /\ kind = "unsupported" /\ stage' = "Refused" /\ failed' = failed \cup {operation} /\ UNCHANGED <<operation,kind,opener,navigate,accepted,notice,live,published,beforeNotice>>
Acknowledge == /\ stage = "ExternalHandoff" /\ stage' = "Accepted" /\ accepted' = accepted \cup {operation} /\ UNCHANGED <<operation,kind,opener,navigate,failed,notice,live,published,beforeNotice>>
OpenerFails == /\ stage = "ExternalHandoff" /\ stage' = "Failed" /\ failed' = failed \cup {operation} /\ UNCHANGED <<operation,kind,opener,navigate,accepted,notice,live,published,beforeNotice>>
AnotherLink == /\ operation = 1 /\ stage \in {"InApp","Accepted","Refused","Failed"} /\ operation' = 2 /\ kind' \in {"own","web","unsupported"} /\ stage' = "ProposedNavigation" /\ opener' = FALSE /\ navigate' = FALSE /\ UNCHANGED <<accepted,failed,notice,live,published,beforeNotice>>
ReceiveNotice == \E link \in failed: /\ live /\ beforeNotice' = notice /\ notice' = link /\ published' = TRUE /\ UNCHANGED <<operation,kind,stage,opener,navigate,accepted,failed,live>>
Dismiss == /\ notice' = 0 /\ beforeNotice' = notice /\ published' = FALSE /\ UNCHANGED <<operation,kind,stage,opener,navigate,accepted,failed,live>>
Retire == /\ live /\ live' = FALSE /\ published' = FALSE /\ UNCHANGED <<operation,kind,stage,opener,navigate,accepted,failed,notice,beforeNotice>>
Next == ClassifyOwn \/ ClassifyWeb \/ Refuse \/ Acknowledge \/ OpenerFails \/ AnotherLink \/ ReceiveNotice \/ Dismiss \/ Retire
OnlyWebToOpener == opener => kind = "web"
KeepExternalOutOfWebview == navigate => kind = "own"
NoticeHasFailureIdentity == notice # 0 => notice \in failed /\ notice \notin accepted
RetiredNoticeCannotPublish == ~live => ~published
AcceptanceNotFailure == accepted \cap failed = {}
Spec == Init /\ [][Next]_vars
====
