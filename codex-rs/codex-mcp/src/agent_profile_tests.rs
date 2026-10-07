use super::*;

fn discovered_servers() -> HashMap<String, ()> {
    ["configured", "plugin", "extension", "ilhae-tools"]
        .into_iter()
        .map(|name| (name.to_owned(), ()))
        .collect()
}

#[test]
fn profile_filters_final_configured_plugin_and_extension_servers() {
    let servers = restrict_servers(
        discovered_servers(),
        parse_selection(Some("[\"configured\",\"ilhae-tools\"]")),
    );
    assert_eq!(
        servers,
        HashMap::from([
            ("configured".to_owned(), ()),
            ("ilhae-tools".to_owned(), ())
        ])
    );
}

#[test]
fn absent_selection_preserves_servers_and_empty_selection_removes_all() {
    assert_eq!(
        restrict_servers(discovered_servers(), parse_selection(None)),
        discovered_servers()
    );
    assert!(restrict_servers(discovered_servers(), parse_selection(Some("[]"))).is_empty());
}

#[test]
fn malformed_selection_cannot_enable_unselected_servers() {
    for raw in ["{}", "[3]", "[\" \"]", "invalid"] {
        assert!(restrict_servers(discovered_servers(), parse_selection(Some(raw))).is_empty());
    }
}
