use super::*;
use crate::LLAMA_SERVER_OSS_PROVIDER_ID;
use crate::config::test_config;
use pretty_assertions::assert_eq;

#[test]
fn reasoning_summaries_override_true_enables_support() {
    let model = model_info_from_slug("unknown-model");
    let mut config = test_config();
    config.model_supports_reasoning_summaries = Some(true);

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.supports_reasoning_summaries = true;

    assert_eq!(updated, expected);
}

#[test]
fn reasoning_summaries_override_false_does_not_disable_support() {
    let mut model = model_info_from_slug("unknown-model");
    model.supports_reasoning_summaries = true;
    let mut config = test_config();
    config.model_supports_reasoning_summaries = Some(false);

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn reasoning_summaries_override_false_is_noop_when_model_is_false() {
    let model = model_info_from_slug("unknown-model");
    let mut config = test_config();
    config.model_supports_reasoning_summaries = Some(false);

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn fallback_local_model_instructions_include_backend_identity() {
    let model = model_info_from_slug("Qwen3.5-27B.Q6_K.gguf");
    let mut config = test_config();
    config.model_provider_id = LLAMA_SERVER_OSS_PROVIDER_ID.to_string();

    let updated = with_config_overrides(model, &config);

    assert!(updated.base_instructions.contains("Qwen3.5-27B.Q6_K.gguf"));
    assert!(
        updated
            .base_instructions
            .contains(LLAMA_SERVER_OSS_PROVIDER_ID)
    );
    assert!(
        updated
            .base_instructions
            .contains("Do not claim to be GPT-5.4")
    );
    assert_eq!(updated.model_messages, None);
}
