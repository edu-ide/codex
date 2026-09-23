use super::*;

#[test]
fn local_model_selection_never_reads_cloud_configuration() {
    let mut profile = IlhaeProfileConfig::default();
    profile.agent.engine_id = Some("laya-router".into());
    assert_eq!(
        resolve_profile_model_name("router", &profile, || panic!(
            "local router read cloud config"
        )),
        "laya-router"
    );
    profile.native_runtime.model_path = "/models/Bonsai.gguf".into();
    assert_eq!(
        resolve_profile_model_name("router", &profile, || panic!(
            "local model read cloud config"
        )),
        "Bonsai"
    );
}

#[test]
fn cloud_model_selection_uses_explicit_compatibility_input() {
    let mut profile = IlhaeProfileConfig::default();
    profile.agent.engine_id = Some("openai".into());
    assert_eq!(
        resolve_profile_model_name("cloud", &profile, || Some("configured-cloud-model".into())),
        "configured-cloud-model"
    );
}
