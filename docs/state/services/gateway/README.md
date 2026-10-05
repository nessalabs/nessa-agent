---
id: "gateway"
title: "Local gateway"
kind: "service"
status: "implemented"
summary: "The local gateway receives authenticated Nessa commands."
parent: "nessa"
sources:
  - "crates/nessa-server/src/product/socket.rs"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/composition/root.rs"
diagramLinks:
  Product: "client-commands"
  Conversations: "gateway-conversation"
  SDK: "sdk"
  Reads: "gateway-record-reads"
  Attachments: "gateway-attachments"
  MCP: "mcp"
  Auth: "auth"
---

# Local gateway

The local gateway receives authenticated Nessa commands. It owns conversations, checks access and coordinates agents, stored history, images and app tool calls.

It can accept work before an agent is ready. Current views, command replies and deletion progress each describe different parts of that work.

## Owner map

These boxes open related pages. They are parts of the system, rather than mutually exclusive runtime states.

```mermaid
flowchart TB
    Product[Authenticated product socket]
    Conversations[Conversation identity and deletion]
    SDK[SDK attachment and work]
    Reads[Bounded physical history reads]
    Attachments[Attachment ownership]
    MCP[MCP sessions and apps]
    Auth[Identity and access]
    Product -->|fresh admission| Auth
    Product --> Conversations
    Product --> Reads
    Conversations -->|one Agent per live slot| SDK
    Conversations --> Attachments
    Conversations --> MCP
```

## Browse this area

- [Attachments and content](attachments/README.md)
- [Conversation identity and deletion](conversation.md)
- [Bounded physical history reads](record-reads.md)
