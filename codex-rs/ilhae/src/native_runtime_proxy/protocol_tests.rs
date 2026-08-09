use super::*;

#[test]
fn ensure_request_round_trip_preserves_complete_runtime_specification() {
    let mut config = IlhaeProfileNativeRuntimeConfig {
        enabled: false,
        provider: Some("llama-server".to_string()),
        health_url: "http://127.0.0.1:8081/health".to_string(),
        url: Some("http://127.0.0.1:8081/v1/responses".to_string()),
        base_url: "http://127.0.0.1:8081/v1".to_string(),
        proxy_base_url: Some("http://127.0.0.1:18083/v1".to_string()),
        proxy_control_url: Some("http://127.0.0.1:18083/_ilhae/native-runtime/ensure".to_string()),
        proxy_control_token_env: Some("ILHAE_RUNTIME_PROXY_TOKEN".to_string()),
        ssh_host: Some("yth".to_string()),
        ssh_local_port: Some(18083),
        ssh_remote_port: Some(8083),
        server_bin: "/opt/llama.cpp/llama-server".to_string(),
        model_path: "/models/fable.gguf".to_string(),
        chat_template_file: "/templates/qwen.jinja".to_string(),
        log_file: "/var/log/ilhae/llama.log".to_string(),
        http_headers: Some(std::collections::BTreeMap::from([(
            "X-Runtime-Mode".to_string(),
            "full".to_string(),
        )])),
        env_http_headers: Some(std::collections::BTreeMap::from([(
            "Authorization".to_string(),
            "ILHAE_UPSTREAM_AUTHORIZATION".to_string(),
        )])),
        request_max_retries: Some(7),
        stream_max_retries: Some(9),
        context_window: Some(131_072),
        startup_timeout_secs: 300,
        args: vec![
            "-m".to_string(),
            "/models/fable.gguf".to_string(),
            "-c".to_string(),
            "131072".to_string(),
            "-ngl".to_string(),
            "100".to_string(),
        ],
        ..Default::default()
    };
    config
        .env
        .insert("CUDA_VISIBLE_DEVICES".to_string(), "0".to_string());
    config.query_params = Some(std::collections::BTreeMap::from([(
        "draft".to_string(),
        "mtp".to_string(),
    )]));
    let request = EnsureNativeRuntimeRequest {
        profile_id: "fable-remote".to_string(),
        thinking_mode: "off".to_string(),
        config,
    };

    let encoded = serde_json::to_vec(&request).expect("serialize complete runtime request");
    let decoded: EnsureNativeRuntimeRequest =
        serde_json::from_slice(&encoded).expect("deserialize complete runtime request");

    assert_eq!(decoded, request);
}
