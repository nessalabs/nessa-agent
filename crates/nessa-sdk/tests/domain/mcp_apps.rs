//! MCP Apps values: what a server declares about a tool's UI, and the UI
//! resource itself. Each bound and each alphabet is tested at its edge.
use nessa_sdk::domain::agent_execution::tools::McpTool;
use nessa_sdk::domain::mcp_apps::{
    ListedTool, McpAppError, ToolHints, ToolUi, UiCsp, UiPermissions, UiResource, UiResourceUri,
    UiVisibility, MAX_CSP_SOURCES, MAX_CSP_SOURCE_BYTES, MAX_UI_HTML_BYTES, MAX_UI_URI_BYTES,
};

fn tool(server: &str, name: &str) -> McpTool {
    McpTool::new(server, name).unwrap()
}
fn uri(value: &str) -> UiResourceUri {
    UiResourceUri::new(value).unwrap()
}
fn ui(value: &str) -> ToolUi {
    ToolUi::new(Some(uri(value)), UiVisibility::BOTH)
}

#[test]
fn a_ui_uri_is_ui_scheme_bounded_and_one_line() {
    assert_eq!(uri("ui://chart/a.html").as_str(), "ui://chart/a.html");
    assert_eq!(UiResourceUri::SCHEME, "ui://");
    for refused in ["https://chart/a.html", "UI://chart", "ui://", "", "ui:/x"] {
        assert_eq!(
            UiResourceUri::new(refused),
            Err(McpAppError::NotUiUri),
            "{refused}"
        );
    }
    for refused in ["ui://a b", "ui://a\nb", "ui://a\u{7}", "ui://a\u{a0}"] {
        assert_eq!(
            UiResourceUri::new(refused),
            Err(McpAppError::InvalidValue("UI resource URI")),
            "{refused:?}"
        );
    }
    let longest = format!("ui://{}", "a".repeat(MAX_UI_URI_BYTES - 5));
    assert_eq!(uri(&longest).as_str().len(), MAX_UI_URI_BYTES);
    assert_eq!(
        UiResourceUri::new(format!("{longest}a")),
        Err(McpAppError::ValueTooLong {
            field: "UI resource URI",
            max_bytes: MAX_UI_URI_BYTES
        })
    );
    // Counted in bytes: a multibyte character can tip it over.
    let multibyte = format!("ui://{}é", "a".repeat(MAX_UI_URI_BYTES - 6));
    assert!(matches!(
        UiResourceUri::new(multibyte),
        Err(McpAppError::ValueTooLong { .. })
    ));
}

#[test]
fn visibility_says_who_sees_the_tool() {
    assert!(UiVisibility::BOTH.model() && UiVisibility::BOTH.app());
    let app_only = UiVisibility::new(false, true);
    assert!(!app_only.model() && app_only.app());
    let model_only = UiVisibility::new(true, false);
    assert!(model_only.model() && !model_only.app());
    let declared = ToolUi::new(Some(uri("ui://x/y")), app_only);
    assert_eq!(declared.resource_uri().unwrap().as_str(), "ui://x/y");
    assert_eq!(declared.visibility(), app_only);
    // Visibility stands without a UI (#412); saying nothing is no UI, for both.
    let model_only_no_ui = ToolUi::new(None, model_only);
    assert_eq!(model_only_no_ui.resource_uri(), None);
    assert_eq!(model_only_no_ui.visibility(), model_only);
    assert_eq!(ToolUi::default(), ToolUi::new(None, UiVisibility::BOTH));
}

#[test]
fn a_call_names_a_listed_tool_exactly_or_in_the_harness_spelling() {
    // Exactly, on the same server.
    assert!(tool("mcptest", "rows.get").names(&tool("mcptest", "rows.get")));
    // Claude's harness turns every character outside [A-Za-z0-9_-] into `_`.
    assert!(tool("mcptest", "rows_get").names(&tool("mcptest", "rows.get")));
    assert!(tool("mcptest", "a-b_c9").names(&tool("mcptest", "a-b_c9")));
    assert!(tool("s", "x__y").names(&tool("s", "x/:y")));
    // Per UTF-16 code unit: one for é, two for a character outside the BMP.
    assert!(tool("s", "caf_").names(&tool("s", "café")));
    assert!(tool("s", "a__").names(&tool("s", "a😀")));
    assert!(!tool("s", "a_").names(&tool("s", "a😀")));
    // Another server, another tool, a prefix, or a longer name are not it.
    assert!(!tool("other", "rows.get").names(&tool("mcptest", "rows.get")));
    assert!(!tool("mcptest", "rows_ge").names(&tool("mcptest", "rows.get")));
    assert!(!tool("mcptest", "rows_gets").names(&tool("mcptest", "rows.get")));
    assert!(!tool("mcptest", "rows.get").names(&tool("mcptest", "rows_get")));
    assert!(!tool("mcptest", "rowsXget").names(&tool("mcptest", "rows.get")));
}

#[test]
fn the_ui_for_a_call_is_the_one_named_tools_or_none() {
    let listed = vec![
        ListedTool::new(tool("s", "show_chart"), ui("ui://s/chart.html")),
        ListedTool::new(tool("s", "report"), ToolUi::default()),
        ListedTool::new(tool("s", "rows.get"), ui("ui://s/rows.html")),
        ListedTool::new(tool("s", "rows_get"), ui("ui://s/other.html")),
    ];
    assert_eq!(listed[0].tool(), &tool("s", "show_chart"));
    assert_eq!(listed[1].ui(), &ToolUi::default());
    let chart = ListedTool::ui_for(&listed, &tool("s", "show_chart")).unwrap();
    assert_eq!(chart.resource_uri().unwrap().as_str(), "ui://s/chart.html");
    // Named, but declared no UI.
    assert_eq!(
        ListedTool::ui_for(&listed, &tool("s", "report")),
        Some(&ToolUi::default())
    );
    // Named by nothing listed, or on another server.
    assert_eq!(ListedTool::ui_for(&listed, &tool("s", "missing")), None);
    assert_eq!(ListedTool::ui_for(&listed, &tool("t", "show_chart")), None);
    // `rows_get` names both `rows.get` and `rows_get`: no guess.
    assert_eq!(ListedTool::ui_for(&listed, &tool("s", "rows_get")), None);
    // `rows.get` names only itself.
    assert_eq!(
        ListedTool::ui_for(&listed, &tool("s", "rows.get"))
            .unwrap()
            .resource_uri()
            .unwrap()
            .as_str(),
        "ui://s/rows.html"
    );
}

#[test]
fn a_side_is_included_only_when_every_declaration_includes_it() {
    let both = UiVisibility::BOTH;
    let app = UiVisibility::new(false, true);
    let model = UiVisibility::new(true, false);
    let nobody = UiVisibility::new(false, false);
    assert_eq!(both.every(app), app);
    assert_eq!(app.every(both), app);
    assert_eq!(both.every(model), model);
    assert_eq!(model.every(both), model);
    assert_eq!(app.every(model), nobody);
    assert_eq!(model.every(app), nobody);
    assert_eq!(nobody.every(both), nobody);
    assert_eq!(both.every(nobody), nobody);
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn csp_sources_are_bounded_and_cannot_end_a_directive() {
    let csp = UiCsp::new(
        strings(&["https://api.example.com"]),
        strings(&["https://*.cdn.example.com:443/path?x=1#y", "data:"]),
        strings(&["https://frame.example.com"]),
        strings(&["https://base.example.com"]),
    )
    .unwrap();
    assert_eq!(&*csp.connect_domains()[0], "https://api.example.com");
    assert_eq!(csp.resource_domains().len(), 2);
    assert_eq!(&*csp.frame_domains()[0], "https://frame.example.com");
    assert_eq!(&*csp.base_uri_domains()[0], "https://base.example.com");
    assert_eq!(UiCsp::default().connect_domains().len(), 0);

    for refused in [
        "a b", "a;b", "a,b", "'self'", "\"x\"", "", "a\nb", "é.com", "a<b",
    ] {
        let field = "CSP connectDomains";
        assert_eq!(
            UiCsp::new(strings(&[refused]), vec![], vec![], vec![]),
            Err(McpAppError::InvalidValue(field)),
            "{refused:?}"
        );
    }
    // Each list is named in its refusal.
    let bad = || strings(&["a b"]);
    assert_eq!(
        UiCsp::new(vec![], bad(), vec![], vec![]),
        Err(McpAppError::InvalidValue("CSP resourceDomains"))
    );
    assert_eq!(
        UiCsp::new(vec![], vec![], bad(), vec![]),
        Err(McpAppError::InvalidValue("CSP frameDomains"))
    );
    assert_eq!(
        UiCsp::new(vec![], vec![], vec![], bad()),
        Err(McpAppError::InvalidValue("CSP baseUriDomains"))
    );
    let at_bound = "a".repeat(MAX_CSP_SOURCE_BYTES);
    assert!(UiCsp::new(vec![at_bound.clone()], vec![], vec![], vec![]).is_ok());
    assert_eq!(
        UiCsp::new(vec![format!("{at_bound}a")], vec![], vec![], vec![]),
        Err(McpAppError::ValueTooLong {
            field: "CSP connectDomains",
            max_bytes: MAX_CSP_SOURCE_BYTES
        })
    );
    let full = vec!["a".to_owned(); MAX_CSP_SOURCES];
    assert!(UiCsp::new(full.clone(), vec![], vec![], vec![]).is_ok());
    let mut over = full;
    over.push("a".into());
    assert_eq!(
        UiCsp::new(over, vec![], vec![], vec![]),
        Err(McpAppError::TooManyValues {
            field: "CSP connectDomains",
            max: MAX_CSP_SOURCES
        })
    );
}

#[test]
fn a_ui_resource_bounds_its_html_and_domain() {
    let permissions = UiPermissions {
        camera: true,
        clipboard_write: true,
        ..UiPermissions::default()
    };
    let resource = UiResource::new(
        uri("ui://s/app.html"),
        "<p>app</p>".into(),
        UiCsp::default(),
        permissions,
        Some("https://app.example.com".into()),
        Some(true),
    )
    .unwrap();
    assert_eq!(resource.uri().as_str(), "ui://s/app.html");
    assert_eq!(resource.html(), "<p>app</p>");
    assert_eq!(resource.csp(), &UiCsp::default());
    assert!(resource.permissions().camera && !resource.permissions().microphone);
    assert!(!resource.permissions().geolocation && resource.permissions().clipboard_write);
    assert_eq!(resource.domain(), Some("https://app.example.com"));
    assert_eq!(resource.prefers_border(), Some(true));

    let make = |html: String, domain: Option<&str>| {
        UiResource::new(
            uri("ui://s/app.html"),
            html,
            UiCsp::default(),
            UiPermissions::default(),
            domain.map(str::to_owned),
            None,
        )
    };
    let at_bound = "a".repeat(MAX_UI_HTML_BYTES);
    let unbordered = make(at_bound.clone(), None).unwrap();
    assert_eq!(
        (unbordered.domain(), unbordered.prefers_border()),
        (None, None)
    );
    assert_eq!(
        make(format!("{at_bound}a"), None),
        Err(McpAppError::ValueTooLong {
            field: "UI resource HTML",
            max_bytes: MAX_UI_HTML_BYTES
        })
    );
    assert_eq!(
        make(String::new(), Some("a;b")),
        Err(McpAppError::InvalidValue("UI domain"))
    );
}

#[test]
fn refusals_say_what_was_refused() {
    let shown = [
        (McpAppError::NotUiUri, "not a ui:// URI"),
        (
            McpAppError::ValueTooLong {
                field: "x",
                max_bytes: 3,
            },
            "x is longer than 3 bytes",
        ),
        (
            McpAppError::TooManyValues { field: "y", max: 2 },
            "y has more than 2 entries",
        ),
        (
            McpAppError::InvalidValue("z"),
            "z is empty or holds a character it may not",
        ),
    ];
    for (error, text) in shown {
        assert_eq!(error.to_string(), text);
        let _: &dyn std::error::Error = &error;
    }
}

#[test]
fn a_tool_is_destructive_unless_it_says_it_only_reads_or_destroys_nothing() {
    // MCP's defaults: silence is destructive.
    for (read_only, destructive, expected) in [
        (None, None, true),
        (Some(false), None, true),
        (None, Some(true), true),
        (Some(false), Some(true), true),
        (Some(true), None, false),
        (None, Some(false), false),
        (Some(true), Some(true), false),
        (Some(false), Some(false), false),
    ] {
        assert_eq!(
            ToolHints::new(read_only, destructive).destructive(),
            expected,
            "readOnlyHint {read_only:?}, destructiveHint {destructive:?}"
        );
    }
    // A tool listed without hints is destructive; with them, as they say.
    let listed = ListedTool::new(tool("s", "t"), ToolUi::default());
    assert!(listed.hints().destructive());
    let reads = listed.with_hints(ToolHints::new(Some(true), None));
    assert!(!reads.hints().destructive());
    assert_eq!(reads.tool(), &tool("s", "t"));
}

#[test]
fn hints_read_back_exactly_as_the_tool_gave_them() {
    // Every pairing, so each getter is shown to read its own hint and not
    // the other's.
    let said = [Some(true), Some(false), None];
    for read_only in said {
        for destructive in said {
            let hints = ToolHints::new(read_only, destructive);
            assert_eq!(hints.read_only_hint(), read_only, "readOnlyHint");
            assert_eq!(hints.destructive_hint(), destructive, "destructiveHint");
        }
    }
    // A tool listed without hints said nothing of either.
    let listed = ListedTool::new(tool("s", "t"), ToolUi::default());
    assert_eq!(listed.hints().read_only_hint(), None);
    assert_eq!(listed.hints().destructive_hint(), None);
}
