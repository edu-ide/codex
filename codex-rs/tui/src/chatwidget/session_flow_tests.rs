use super::*;

#[test]
fn startup_route_map_names_each_answer_model_and_server() {
    let routes: LayaRoutesResponse = serde_json::from_value(serde_json::json!({
        "mode": "route",
        "backends": [
            {"role": "default", "model": "Ternary-Bonsai-2-27B-PQ2_0", "address": "127.0.0.1:8081"},
            {"role": "escalation", "model": "Qwen3.8-Flash-Next", "address": "192.168.219.113:8080"}
        ]
    }))
    .expect("route response");
    let rendered = laya_route_lines(routes, "127.0.0.1:8900").join("\n");

    assert!(rendered.contains("System 1: Laya router @ 127.0.0.1:8900"));
    assert!(rendered.contains("default → Ternary-Bonsai-2-27B-PQ2_0"));
    assert!(rendered.contains("127.0.0.1:8081"));
    assert!(rendered.contains("escalation → Qwen3.8-Flash-Next"));
    assert!(rendered.contains("192.168.219.113:8080"));
    assert!(rendered.contains("selection is per request"));
}

#[test]
fn advisor_route_map_does_not_describe_advisor_as_answer_route() {
    let routes: LayaRoutesResponse = serde_json::from_value(serde_json::json!({
        "mode": "advisor",
        "backends": [
            {"role": "default", "model": "bonsai", "address": "127.0.0.1:8081"},
            {"role": "advisor", "model": "flash", "address": "192.168.219.113:8080"}
        ]
    }))
    .expect("route response");

    let rendered = laya_route_lines(routes, "127.0.0.1:8900").join("\n");
    assert!(rendered.contains("Executor and advisor endpoints"));
    assert!(rendered.contains("advisor → flash"));
    assert!(!rendered.contains("Answer routes"));
}
