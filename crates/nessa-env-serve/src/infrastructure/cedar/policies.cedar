// Who may lease what from this host. The principal is the peer leasing:
// `kind` is "ssh" (whoever runs `nessa env serve` over SSH) or "gateway" (a
// paired gateway this host dials), and `hasEntry` says whether the host's
// owner wrote it an entry in grants.json. Both are facts a policy may use; none
// does today, and a test holds that whether an entry was written never decides.
// `allAgents`, `agents` and `commands` are the grant the peer holds: its
// entry as written, or, with none, what the server's domain says a peer
// with no entry holds (`domain::unwritten`: everything for ssh, nothing for
// a dialed gateway). The defaults live there only; the permits below decide
// every peer alike. Nothing is allowed unless a permit says so.

// A grant of every agent.
@id("grant.every_agent")
permit (
    principal is NessaEnv::Peer,
    action == NessaEnv::Action::"agent.lease",
    resource is NessaEnv::Agent
)
when { principal.allAgents };

// A grant naming the agent asked.
@id("grant.named_agent")
permit (
    principal is NessaEnv::Peer,
    action == NessaEnv::Action::"agent.lease",
    resource is NessaEnv::Agent
)
when { principal.agents.contains(resource.name) };

// A grant of commands.
@id("grant.commands")
permit (
    principal is NessaEnv::Peer,
    action == NessaEnv::Action::"command.run",
    resource is NessaEnv::Command
)
when { principal.commands };

// Its gateway's build, run in place of this host's own, once that gateway
// refused this build's protocol. Granted with commands: a peer that may run
// any command here as this account can already replace this build, so this
// gives it nothing more. A peer granted only agents, whose harnesses may be
// sandboxed, is never sent a build to run outside them.
@id("grant.commands_update")
permit (
    principal is NessaEnv::Peer,
    action == NessaEnv::Action::"build.update",
    resource is NessaEnv::Build
)
when { principal.commands };
