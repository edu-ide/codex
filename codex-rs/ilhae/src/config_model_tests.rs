use super::*;
use codex_protocol::openai_models::ModelVisibility;

#[test]
fn custom_router_model_does_not_inherit_codex_cloud_model() {
    let mut router = IlhaeProfileConfig::default();
    router.agent.engine_id = Some("laya-router".to_string());
    router.agent.command = Some("ilhae".to_string());

    assert_eq!(
        resolve_ilhae_profile_model_name("router", &router),
        "laya-router"
    );

    router.native_runtime.model_path = "/models/Bonsai-2-27B.gguf".to_string();
    assert_eq!(
        resolve_ilhae_profile_model_name("router", &router),
        "Bonsai-2-27B"
    );
}

#[test]
fn router_projection_lists_local_models_without_openai_catalog() {
    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("laya-router".to_string());
    let mut router = IlhaeProfileConfig::default();
    router.agent.engine_id = Some("laya-router".to_string());
    config.profiles.insert("laya-router".to_string(), router);

    let mut local = IlhaeProfileConfig::default();
    local.native_runtime.model_path = "/models/Bonsai-2-27B.gguf".to_string();
    local.native_runtime.base_url = "http://127.0.0.1:8081/v1".to_string();
    config.profiles.insert("bonsai-local".to_string(), local);

    let mut cloud = IlhaeProfileConfig::default();
    cloud.agent.engine_id = Some("openai".to_string());
    config.profiles.insert("chatgpt".to_string(), cloud);
    let document: toml::Value = toml::from_str(
        r#"
model = "gpt-5.6"
[model_providers.laya-router]
base_url = "http://127.0.0.1:8900/v1"
requires_openai_auth = false
"#,
    )
    .expect("provider config");
    let catalog_path = Path::new("/tmp/ilhae-models.json");
    let projected = default_ilhae_codex_home_table(&config, &document, catalog_path);
    assert_eq!(
        ["model", "model_provider", "model_catalog_json"].map(|key| projected[key].as_str()),
        [
            Some("laya-router"),
            Some("laya-router"),
            catalog_path.to_str()
        ]
    );
    assert!(
        projected["profiles"]["chatgpt"]
            .get("model_catalog_json")
            .is_none()
    );
    let catalog = native_model_catalog(&config, &document).expect("local catalog");
    assert_eq!(
        catalog
            .models
            .iter()
            .map(|model| (
                model.slug.as_str(),
                model.visibility.clone(),
                model.supported_in_api
            ))
            .collect::<Vec<_>>(),
        vec![
            ("Bonsai-2-27B", ModelVisibility::List, true),
            ("laya-router", ModelVisibility::List, true)
        ]
    );
}

#[test]
fn provider_aliases_share_local_model_catalog() {
    let document: toml::Value = toml::from_str(
        r#"
[model_providers.llama-server]
base_url = "http://127.0.0.1:8081/v1"
requires_openai_auth = false
"#,
    )
    .expect("provider config");
    let catalog_path = Path::new("/tmp/ilhae-models.json");

    for (engine, provider, expected_provider) in [
        (None, None, "llama-server"),
        (Some("ilhae"), None, "llama-server"),
        (Some("codex"), None, "llama-server"),
        (Some("laya-router"), Some("ilhae"), "llama-server"),
        (Some("laya-router"), Some("codex"), "llama-server"),
        // Explicit llama-server enables the native proxy by default, whose
        // generated provider owns the catalog instead of the user provider.
        (
            Some("laya-router"),
            Some("llama-server"),
            "ilhae-native-local",
        ),
    ] {
        let mut profile = IlhaeProfileConfig::default();
        profile.agent.engine_id = engine.map(str::to_string);
        profile.native_runtime.provider = provider.map(str::to_string);
        profile.native_runtime.model_path = "/models/Bonsai-2-27B.gguf".to_string();
        let projected =
            codex_profile_table_for_ilhae_profile("local", &profile, &document, catalog_path);
        assert_eq!(
            ["model_provider", "model_catalog_json"].map(|key| projected[key].as_str()),
            [Some(expected_provider), catalog_path.to_str()],
            "engine={engine:?}, provider={provider:?}"
        );

        let mut config = IlhaeTomlConfig::default();
        config.profiles.insert("local".to_string(), profile);
        let catalog = native_model_catalog(&config, &document).expect("local catalog");
        assert_eq!(
            catalog
                .models
                .iter()
                .map(|model| model.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["Bonsai-2-27B"],
            "engine={engine:?}, provider={provider:?}"
        );
    }
}

#[test]
fn router_catalog_respects_provider_auth_default() {
    let mut profile = IlhaeProfileConfig::default();
    profile.agent.engine_id = Some("laya-router".to_string());
    let mut config = IlhaeTomlConfig::default();
    config
        .profiles
        .insert("router".to_string(), profile.clone());
    let catalog_path = Path::new("/tmp/ilhae-models.json");

    for (auth_field, expects_catalog) in [
        ("", true),
        ("requires_openai_auth = false", true),
        ("requires_openai_auth = true", false),
        ("requires_openai_auth = 'invalid'", false),
    ] {
        let document: toml::Value = toml::from_str(&format!(
            r#"
[model_providers.laya-router]
base_url = "http://127.0.0.1:8900/v1"
{auth_field}
"#
        ))
        .expect("provider config");
        let projected =
            codex_profile_table_for_ilhae_profile("router", &profile, &document, catalog_path);
        assert_eq!(
            projected.get("model_catalog_json").is_some(),
            expects_catalog,
            "auth field: {auth_field:?}"
        );
        assert_eq!(
            native_model_catalog(&config, &document).is_some(),
            expects_catalog,
            "auth field: {auth_field:?}"
        );
    }
}
