// Who may lease what from this host. The principal is the peer leasing:
// `kind` is "ssh" (whoever runs `nessa env serve` over SSH) or "gateway" (a
// paired gateway this host dials); `hasEntry` says whether the host's owner
// wrote it an entry in grants.json, and `allAgents`, `agents` and
// `commands` are that entry as written (false and empty without one).
// Nothing is allowed unless a permit below says so.

// SSH with no entry keeps what a host served over SSH always had: every
// agent the host runs.
@id("ssh.unnamed.every_agent")
permit (
    principal is NessaEnv::Peer,
    action == NessaEnv::Action::"agent.lease",
    resource is NessaEnv::Agent
)
when { principal.kind == "ssh" && !principal.hasEntry };

// ... and commands, when the host runs them.
@id("ssh.unnamed.commands")
permit (
    principal is NessaEnv::Peer,
    action == NessaEnv::Action::"command.run",
    resource is NessaEnv::Command
)
when { principal.kind == "ssh" && !principal.hasEntry };

// Any peer but SSH that the owner never named is granted nothing.
@id("unnamed.nothing")
forbid (
    principal is NessaEnv::Peer,
    action,
    resource
)
when { principal.kind != "ssh" && !principal.hasEntry };

// An entry granting every agent.
@id("entry.every_agent")
permit (
    principal is NessaEnv::Peer,
    action == NessaEnv::Action::"agent.lease",
    resource is NessaEnv::Agent
)
when { principal.hasEntry && principal.allAgents };

// An entry naming the agent asked.
@id("entry.named_agent")
permit (
    principal is NessaEnv::Peer,
    action == NessaEnv::Action::"agent.lease",
    resource is NessaEnv::Agent
)
when { principal.hasEntry && principal.agents.contains(resource.name) };

// An entry granting commands.
@id("entry.commands")
permit (
    principal is NessaEnv::Peer,
    action == NessaEnv::Action::"command.run",
    resource is NessaEnv::Command
)
when { principal.hasEntry && principal.commands };
