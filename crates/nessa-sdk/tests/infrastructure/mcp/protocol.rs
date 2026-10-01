//! The handshake, `tools/list`, `resources/read` and a gateway request's
//! orderings ("A gateway request" table).
use super::fixture::{Behaviour, CHART, CHART_HTML};
use super::{servers, session};
use crate::domain::agent_execution::tools::McpTool;
use crate::domain::mcp_apps::{UiResourceUri, UiVisibility, APP_MIME_TYPE, MAX_UI_HTML_BYTES};
use crate::infrastructure::mcp::{
    connection::{Connection, MAX_IN_FLIGHT},
    McpError, MAX_TOOLS, MAX_TOOL_PAGES, REQUEST_TIMEOUT,
};
use base64::Engine;
use serde_json::json;
use std::sync::Arc;

fn chart() -> UiResourceUri {
    UiResourceUri::new(CHART).unwrap()
}

#[tokio::test]
async fn the_handshake_declares_the_mcp_apps_extension_and_lists_the_tools() {
    let (session, servers, launcher, _) = session(Behaviour::default()).await;
    let tools = session.list_tools().await.unwrap();
    let server = launcher.server(0);
    let initialize = &server.with_method("initialize")[0];
    assert_eq!(initialize["params"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        initialize["params"]["capabilities"],
        json!({ "extensions": { "io.modelcontextprotocol/ui": { "mimeTypes": [APP_MIME_TYPE] } } })
    );
    assert_eq!(initialize["params"]["clientInfo"]["name"], "nessa");
    // Initialized once, before anything else is asked.
    let methods: Vec<_> = server
        .received()
        .iter()
        .map(|m| m["method"].clone())
        .collect();
    assert_eq!(
        methods[..3],
        [
            json!("initialize"),
            json!("notifications/initialized"),
            json!("tools/list")
        ]
    );
    assert_eq!(tools.len(), 2);
    assert_eq!(
        tools[0].tool(),
        &McpTool::new("fixture", "show_chart").unwrap()
    );
    assert_eq!(tools[0].ui().unwrap().resource_uri(), &chart());
    assert_eq!(tools[0].ui().unwrap().visibility(), UiVisibility::BOTH);
    // A tool without UI.
    assert_eq!(tools[1].ui(), None);
    // Kept for the view: the call names `show_chart`, which has a UI.
    let call = McpTool::new("fixture", "show_chart").unwrap();
    assert_eq!(servers.tool_ui(&call).unwrap().resource_uri(), &chart());
    assert_eq!(
        servers.tool_ui(&McpTool::new("fixture", "report").unwrap()),
        None
    );
    assert_eq!(
        servers.tool_ui(&McpTool::new("other", "show_chart").unwrap()),
        None
    );
}

#[tokio::test]
async fn an_unsupported_version_or_a_refused_initialize_is_a_handshake_failure() {
    for version in [Some("2099-01-01"), None] {
        let (servers, launcher, _) = servers(Behaviour {
            version,
            ..Behaviour::default()
        });
        assert!(matches!(
            servers.open("fixture").await,
            Err(McpError::Handshake(_))
        ));
        // Its process is stopped: the fixture sees its input close.
        launcher.server(0).stopped().await;
    }
    // Each version this client accepts.
    for version in ["2025-06-18", "2025-03-26", "2024-11-05"] {
        let (servers, _, _) = servers(Behaviour {
            version: Some(version),
            ..Behaviour::default()
        });
        assert!(servers.open("fixture").await.is_ok(), "{version}");
    }
}

#[tokio::test]
async fn tools_are_read_across_pages_and_unreadable_ones_are_handled_one_by_one() {
    let (session, _, _, _) = session(Behaviour {
        pages: vec![
            vec![
                json!({ "name": "a", "_meta": { "ui": { "resourceUri": "ui://f/a", "visibility": ["app"] } } }),
                // A name that cannot be one: left out.
                json!({ "name": "has space" }),
                json!({ "description": "no name" }),
            ],
            vec![
                // `_meta.ui` that cannot be read: kept without a UI.
                json!({ "name": "b", "_meta": { "ui": { "resourceUri": "https://not-ui" } } }),
                json!({ "name": "c", "_meta": { "ui": { "resourceUri": "ui://f/c", "visibility": "app" } } }),
                json!({ "name": "d", "_meta": { "ui": { "visibility": ["model"] } } }),
                json!({ "name": "e", "_meta": { "ui": { "resourceUri": "ui://f/e", "visibility": ["model", "other"] } } }),
            ],
        ],
        ..Behaviour::default()
    })
    .await;
    let tools = session.list_tools().await.unwrap();
    let names: Vec<_> = tools
        .iter()
        .map(|tool| tool.tool().tool().to_owned())
        .collect();
    assert_eq!(names, ["a", "b", "c", "d", "e"]);
    assert_eq!(
        tools[0].ui().unwrap().visibility(),
        UiVisibility::new(false, true)
    );
    assert_eq!(tools[1].ui(), None);
    assert_eq!(tools[2].ui(), None);
    assert_eq!(tools[3].ui(), None);
    assert_eq!(
        tools[4].ui().unwrap().visibility(),
        UiVisibility::new(true, false)
    );
}

#[tokio::test]
async fn a_list_past_its_bounds_or_of_the_wrong_shape_is_refused_and_not_kept() {
    let many = |count: usize| {
        (0..count)
            .map(|n| json!({ "name": format!("t{n}") }))
            .collect::<Vec<_>>()
    };
    let (session, _, _, _) = session(Behaviour {
        pages: vec![many(MAX_TOOLS), many(1)],
        ..Behaviour::default()
    })
    .await;
    assert_eq!(
        session.list_tools().await,
        Err(McpError::TooLarge("tools/list"))
    );
    let (session, _, _, _) = super::session(Behaviour {
        pages: vec![many(1); MAX_TOOL_PAGES + 1],
        ..Behaviour::default()
    })
    .await;
    assert_eq!(
        session.list_tools().await,
        Err(McpError::TooLarge("tools/list"))
    );
    // Exactly at the bounds is a list.
    let (session, _, _, _) = super::session(Behaviour {
        pages: vec![many(MAX_TOOLS / MAX_TOOL_PAGES); MAX_TOOL_PAGES],
        ..Behaviour::default()
    })
    .await;
    assert_eq!(session.list_tools().await.unwrap().len(), MAX_TOOLS);
}

#[tokio::test]
async fn an_app_resource_is_read_with_what_it_asks_of_the_host() {
    let (session, _, _, _) = session(Behaviour::default()).await;
    let app = session.read_ui_resource(&chart()).await.unwrap();
    assert_eq!(app.uri(), &chart());
    assert_eq!(app.html(), CHART_HTML);
    assert_eq!(&*app.csp().connect_domains()[0], "https://api.example.com");
    assert!(app.csp().resource_domains().is_empty());
    assert!(app.permissions().camera && !app.permissions().microphone);
    assert_eq!(app.prefers_border(), Some(true));
    assert_eq!(app.domain(), None);
}

/// A fixture serving `content` (with `uri` filled in) at the chart's URI.
async fn read(content: serde_json::Value) -> Result<crate::domain::mcp_apps::UiResource, McpError> {
    let mut behaviour = Behaviour::default();
    behaviour.resources.insert(CHART.into(), content);
    let (session, _, _, _) = session(behaviour).await;
    session.read_ui_resource(&chart()).await
}

#[tokio::test]
async fn a_resource_that_is_not_an_app_or_is_too_large_is_refused_by_type() {
    // Not text/html;profile=mcp-app.
    assert_eq!(
        read(json!({ "uri": CHART, "mimeType": "text/html", "text": "<p>" })).await,
        Err(McpError::NotAnApp)
    );
    assert_eq!(
        read(json!({ "uri": CHART, "text": "<p>" })).await,
        Err(McpError::NotAnApp)
    );
    // Past the HTML bound, as text and as a blob.
    let large = "a".repeat(MAX_UI_HTML_BYTES + 1);
    assert_eq!(
        read(json!({ "uri": CHART, "mimeType": APP_MIME_TYPE, "text": large })).await,
        Err(McpError::TooLarge("UI resource HTML"))
    );
    let blob = base64::engine::general_purpose::STANDARD.encode(&large);
    assert_eq!(
        read(json!({ "uri": CHART, "mimeType": APP_MIME_TYPE, "blob": blob })).await,
        Err(McpError::TooLarge("UI resource HTML"))
    );
    // A UTF-8 blob is the same app.
    let blob = base64::engine::general_purpose::STANDARD.encode("<p>blob</p>");
    let app = read(json!({ "uri": CHART, "mimeType": APP_MIME_TYPE, "blob": blob }))
        .await
        .unwrap();
    assert_eq!(app.html(), "<p>blob</p>");
    // Past a CSP bound.
    let many = vec!["a"; 65];
    assert_eq!(
        read(json!({ "uri": CHART, "mimeType": APP_MIME_TYPE, "text": "",
                     "_meta": { "ui": { "csp": { "frameDomains": many } } } }))
        .await,
        Err(McpError::TooLarge("CSP frameDomains"))
    );
}

#[tokio::test]
async fn a_resource_of_the_wrong_shape_is_malformed() {
    let app = |extra: serde_json::Value| {
        let mut content = json!({ "uri": CHART, "mimeType": APP_MIME_TYPE, "text": "<p>" });
        content
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        content
    };
    for content in [
        // For another URI only.
        json!({ "uri": "ui://fixture/other", "mimeType": APP_MIME_TYPE, "text": "<p>" }),
        json!({ "uri": CHART, "mimeType": APP_MIME_TYPE }),
        json!({ "uri": CHART, "mimeType": APP_MIME_TYPE, "text": "a", "blob": "YQ==" }),
        json!({ "uri": CHART, "mimeType": APP_MIME_TYPE, "blob": "not base64!" }),
        json!({ "uri": CHART, "mimeType": APP_MIME_TYPE, "blob": "/w==" }),
        app(json!({ "_meta": { "ui": { "csp": { "connectDomains": "x" } } } })),
        app(json!({ "_meta": { "ui": { "csp": { "connectDomains": [1] } } } })),
        app(json!({ "_meta": { "ui": { "csp": { "connectDomains": ["a b"] } } } })),
        app(json!({ "_meta": { "ui": { "permissions": [] } } })),
        app(json!({ "_meta": { "ui": { "domain": 1 } } })),
        app(json!({ "_meta": { "ui": { "domain": "a;b" } } })),
        app(json!({ "_meta": { "ui": { "prefersBorder": "yes" } } })),
    ] {
        assert!(
            matches!(read(content.clone()).await, Err(McpError::Malformed(_))),
            "{content}"
        );
    }
    // Every optional field, present.
    let full = read(app(json!({ "_meta": { "ui": {
        "csp": { "connectDomains": [], "resourceDomains": ["https://cdn"], "frameDomains": ["https://f"],
                 "baseUriDomains": ["https://b"] },
        "permissions": { "camera": false, "microphone": {}, "geolocation": true,
                         "clipboardWrite": {}, "unknown": {} },
        "domain": "https://app.example", "prefersBorder": false } } })))
    .await
    .unwrap();
    assert_eq!(&*full.csp().base_uri_domains()[0], "https://b");
    let permissions = full.permissions();
    assert!(
        !permissions.camera
            && permissions.microphone
            && permissions.geolocation
            && permissions.clipboard_write
    );
    assert_eq!(full.domain(), Some("https://app.example"));
    assert_eq!(full.prefers_border(), Some(false));
}

#[tokio::test]
async fn a_server_error_answer_is_remote() {
    let (session, servers, _, _) = session(Behaviour::default()).await;
    let missing = UiResourceUri::new("ui://fixture/missing").unwrap();
    assert_eq!(
        session.read_ui_resource(&missing).await,
        Err(McpError::Remote {
            code: -32002,
            message: "not found".into()
        })
    );
    assert!(matches!(
        servers.open("absent").await,
        Err(McpError::NotConfigured)
    ));
}

#[tokio::test]
async fn an_unanswered_request_times_out_is_cancelled_upstream_and_the_connection_stays() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("resources/read");
    let (session, _, launcher, clock) = session(behaviour).await;
    session.list_tools().await.unwrap();
    let uri = chart();
    let read = session.read_ui_resource(&uri);
    let timeout = clock.passing(|wait| wait.limit() == REQUEST_TIMEOUT, read);
    assert_eq!(timeout.await, Err(McpError::Timeout));
    let server = launcher.server(0);
    server.arrived("notifications/cancelled", 1).await;
    let asked = &server.with_method("resources/read")[0];
    let cancelled = &server.with_method("notifications/cancelled")[0];
    assert_eq!(cancelled["params"]["requestId"], asked["id"]);
    // A late answer to it is dropped, and the connection still serves.
    server.send(json!({ "jsonrpc": "2.0", "id": asked["id"], "result": { "contents": [] } }));
    assert_eq!(session.list_tools().await.unwrap().len(), 2);
    assert_eq!(launcher.launches(), 1);
}

#[tokio::test]
async fn an_answer_with_neither_result_nor_error_is_malformed_for_that_request_only() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("resources/read");
    let (session, _, launcher, _) = session(behaviour).await;
    session.list_tools().await.unwrap();
    let server = launcher.server(0);
    let read = tokio::spawn({
        let session = session.clone();
        async move { session.read_ui_resource(&chart()).await }
    });
    server.arrived("resources/read", 1).await;
    let id = server.with_method("resources/read")[0]["id"].clone();
    // Ids this client never used, and a string id, are ignored.
    server.send(json!({ "jsonrpc": "2.0", "id": 9999, "result": {} }));
    server.send(json!({ "jsonrpc": "2.0", "id": "x", "result": {} }));
    server.send(json!({ "jsonrpc": "2.0", "id": id }));
    assert!(matches!(read.await.unwrap(), Err(McpError::Malformed(_))));
    // An error answer without an integer code is malformed too.
    let read = tokio::spawn({
        let session = session.clone();
        async move { session.read_ui_resource(&chart()).await }
    });
    server.arrived("resources/read", 2).await;
    let id = server.with_method("resources/read")[1]["id"].clone();
    server.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "message": "no code" } }));
    assert!(matches!(read.await.unwrap(), Err(McpError::Malformed(_))));
    assert!(session.list_tools().await.is_ok());
}

#[tokio::test]
async fn an_oversize_or_non_json_frame_ends_the_session_and_fails_what_waits() {
    for (frame, cause) in [
        (
            vec![b'a'; super::super::framing::MAX_FRAME_BYTES + 1],
            McpError::TooLarge("a frame from the MCP server"),
        ),
        (
            b"not json\n".to_vec(),
            McpError::Malformed("a frame from the MCP server is not JSON".into()),
        ),
    ] {
        let mut behaviour = Behaviour::default();
        behaviour.silent.insert("resources/read");
        let (session, _, launcher, _) = session(behaviour).await;
        session.list_tools().await.unwrap();
        let read = tokio::spawn({
            let session = session.clone();
            async move { session.read_ui_resource(&chart()).await }
        });
        launcher.server(0).arrived("resources/read", 1).await;
        launcher.server(0).send_raw(frame);
        assert_eq!(read.await.unwrap(), Err(cause.clone()));
        // Gone: nothing restarts it, and later requests get its end cause.
        assert_eq!(session.list_tools().await, Err(cause));
        assert_eq!(launcher.launches(), 1);
    }
}

#[tokio::test]
async fn the_servers_own_requests_are_answered_here() {
    let (session, _, launcher, _) = session(Behaviour::default()).await;
    session.list_tools().await.unwrap();
    let server = launcher.server(0);
    server.send(json!({ "jsonrpc": "2.0", "id": "p", "method": "ping" }));
    server.send(
        json!({ "jsonrpc": "2.0", "id": 5, "method": "sampling/createMessage", "params": {} }),
    );
    let pong = server
        .arrived_where(|message| message["id"] == "p" && message.get("method").is_none())
        .await;
    assert_eq!(pong, json!({ "jsonrpc": "2.0", "id": "p", "result": {} }));
    let refused = server
        .arrived_where(|message| message["id"] == 5 && message.get("method").is_none())
        .await;
    assert_eq!(refused["error"]["code"], -32601);
}

#[tokio::test]
async fn a_changed_tool_list_is_read_again() {
    let (session, servers, launcher, _) = session(Behaviour::default()).await;
    session.list_tools().await.unwrap();
    let call = McpTool::new("fixture", "later").unwrap();
    assert_eq!(servers.tool_ui(&call), None);
    let server = launcher.server(0);
    server.set_pages(vec![vec![
        json!({ "name": "later", "_meta": { "ui": { "resourceUri": "ui://fixture/later" } } }),
    ]]);
    server.send(json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while servers.tool_ui(&call).is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the changed list is read");
    assert_eq!(
        servers.tool_ui(&call).unwrap().resource_uri().as_str(),
        "ui://fixture/later"
    );
    // A notice about another list is not a reason to list tools.
    let notice = |method: &str| json!({ "jsonrpc": "2.0", "method": method });
    assert!(crate::infrastructure::mcp::servers::tools_changed(&notice(
        "notifications/tools/list_changed"
    )));
    for other in [
        "notifications/resources/list_changed",
        "notifications/prompts/list_changed",
    ] {
        assert!(!crate::infrastructure::mcp::servers::tools_changed(
            &notice(other)
        ));
    }
}

#[tokio::test]
async fn past_the_in_flight_bound_a_call_is_busy_and_nothing_is_sent() {
    let (client, mut server) = tokio::io::duplex(1024 * 1024);
    let (input, output) = tokio::io::split(client);
    let clock = Arc::new(crate::infrastructure::clock::manual::ManualClock::default());
    let connection = Arc::new(Connection::open(input, output, clock));
    let mut waiting = Vec::new();
    for _ in 0..MAX_IN_FLIGHT {
        let connection = connection.clone();
        waiting.push(tokio::spawn(
            async move { connection.call("slow", None).await },
        ));
    }
    // Read every request off the wire, so all are admitted and sent.
    let mut sent = 0;
    let mut buffer = Vec::new();
    while sent < MAX_IN_FLIGHT {
        use tokio::io::AsyncReadExt;
        let mut chunk = [0; 4096];
        let read = server.read(&mut chunk).await.unwrap();
        buffer.extend_from_slice(&chunk[..read]);
        sent = buffer.iter().filter(|byte| **byte == b'\n').count();
    }
    assert_eq!(connection.call("one-more", None).await, Err(McpError::Busy));
    connection.close(McpError::Stopped);
    for call in waiting {
        assert_eq!(call.await.unwrap(), Err(McpError::Stopped));
    }
    // Admitted after the end: the end's cause, not a wait.
    assert_eq!(connection.call("late", None).await, Err(McpError::Stopped));
    assert_eq!(
        connection.notify("late", None).await,
        Err(McpError::Stopped)
    );
    // A request past the frame bound is refused before it is sent.
    let huge = json!("a".repeat(super::super::framing::MAX_FRAME_BYTES));
    assert_eq!(
        connection.call("big", Some(huge)).await,
        Err(McpError::TooLarge("a JSON-RPC frame"))
    );
}
