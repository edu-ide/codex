use super::*;
use pretty_assertions::assert_eq;

struct TestHandler;

impl ToolHandler for TestHandler {
    type Output = crate::tools::context::FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, _invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        unreachable!("test handler should not be invoked")
    }
}

#[test]
fn handler_looks_up_namespaced_aliases_explicitly() {
    let plain_handler = Arc::new(TestHandler) as Arc<dyn AnyToolHandler>;
    let namespaced_handler = Arc::new(TestHandler) as Arc<dyn AnyToolHandler>;
    let namespace = "mcp__codex_apps__gmail";
    let tool_name = "gmail_get_recent_emails";
    let namespaced_name = tool_handler_key(tool_name, Some(namespace));
    let registry = ToolRegistry::new(
        HashMap::from([
            (tool_name.to_string(), Arc::clone(&plain_handler)),
            (namespaced_name, Arc::clone(&namespaced_handler)),
        ]),
        HashMap::new(),
    );

    let plain = registry.handler(tool_name, /*namespace*/ None);
    let namespaced = registry.handler(tool_name, Some(namespace));
    let missing_namespaced = registry.handler(tool_name, Some("mcp__codex_apps__calendar"));

    assert_eq!(plain.is_some(), true);
    assert_eq!(namespaced.is_some(), true);
    assert_eq!(missing_namespaced.is_none(), true);
    assert!(
        plain
            .as_ref()
            .is_some_and(|handler| Arc::ptr_eq(handler, &plain_handler))
    );
    assert!(
        namespaced
            .as_ref()
            .is_some_and(|handler| Arc::ptr_eq(handler, &namespaced_handler))
    );
}

fn sample_command_meta(name: &str) -> CommandMeta {
    CommandMeta {
        name: name.to_string(),
        help_text: "test meta".to_string(),
        usage_example: None,
        is_experimental: false,
        is_visible: true,
        available_during_task: true,
        category: CommandCategory::System,
        tags: None,
        linked_files: None,
        version: None,
        compatibility: None,
    }
}

#[test]
fn metadata_lookup_handles_namespaced_tool_names() {
    let plain_handler = Arc::new(TestHandler) as Arc<dyn AnyToolHandler>;
    let namespaced_handler = Arc::new(TestHandler) as Arc<dyn AnyToolHandler>;
    let namespace = "mcp__codex_apps__gmail";
    let tool_name = "gmail_get_recent_emails";

    let plain_key = tool_name.to_string();
    let namespaced_key = tool_handler_key(tool_name, Some(namespace));
    let plain_meta = sample_command_meta(tool_name);
    let namespaced_meta = sample_command_meta(&namespaced_key);
    let registry = ToolRegistry::new(
        HashMap::from([
            (plain_key.clone(), Arc::clone(&plain_handler)),
            (namespaced_key.clone(), Arc::clone(&namespaced_handler)),
        ]),
        HashMap::from([
            (plain_key, plain_meta.clone()),
            (namespaced_key.clone(), namespaced_meta.clone()),
        ]),
    );

    assert!(
        registry
            .get_metadata_with_namespace(tool_name, Some(namespace))
            .is_some_and(|meta| meta == &namespaced_meta)
    );
    assert!(
        registry
            .get_metadata_with_namespace(tool_name, None)
            .is_some_and(|meta| meta == &plain_meta)
    );
    assert!(
        registry
            .get_metadata(tool_name)
            .is_some_and(|meta| meta == &plain_meta)
    );
}

#[test]
fn builder_register_handler_with_namespace_stores_namespaced_metadata() {
    let mut registry_builder = ToolRegistryBuilder::new();
    let handler = Arc::new(TestHandler) as Arc<TestHandler>;
    let namespace = Some("namespace");
    let tool_name = "namespace_aware_tool";
    let meta = sample_command_meta("namespace_aware_tool");

    registry_builder.register_handler_with_namespace(
        tool_name,
        handler as Arc<TestHandler>,
        meta.clone(),
        namespace,
    );
    let (_, registry) = registry_builder.build();

    assert!(registry.has_handler(tool_name, namespace));
    assert!(
        registry
            .get_metadata_with_namespace(tool_name, namespace)
            .is_some_and(|actual| actual == &meta)
    );
    assert!(
        registry
            .get_metadata_with_namespace(tool_name, None)
            .is_none()
    );
}
