-- How agents talk (issue #1382). `reply_styles` holds a person's own
-- choices: their Nessa level (channel '') and each channel's, one row a role;
-- no row inherits. `conversation_reply_styles` is the style a conversation
-- was started with, fixed at its creation, so a later change never changes
-- a saved conversation's prompt. A conversation without a row predates this
-- table and is told no style.
CREATE TABLE reply_styles (
    organization_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('coordinator', 'thread')),
    style TEXT NOT NULL CHECK (style IN ('plain', 'off', 'custom')),
    text TEXT CHECK ((style = 'custom') = (text IS NOT NULL) AND length(CAST(text AS BLOB)) <= 4096),
    PRIMARY KEY (organization_id, owner_id, channel, role)
) STRICT, WITHOUT ROWID;
CREATE TABLE conversation_reply_styles (
    conversation TEXT PRIMARY KEY NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('coordinator', 'thread')),
    style TEXT NOT NULL CHECK (style IN ('plain', 'off', 'custom')),
    text TEXT CHECK ((style = 'custom') = (text IS NOT NULL) AND length(CAST(text AS BLOB)) <= 4096)
) STRICT, WITHOUT ROWID;
PRAGMA user_version = 7;
