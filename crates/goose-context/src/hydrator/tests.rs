/// Unit tests for context hydration logic — no database required.
#[cfg(test)]
mod tests {
    use crate::hydrator::{ContextHydrator, HydrationIntent, HydratedContext, DocContextItem, CodeContextItem, OwnershipItem, TestContextItem};

    // ─── Tool curation ───────────────────────────────────────────────────────

    fn make_code_items(langs: &[&str]) -> Vec<CodeContextItem> {
        langs
            .iter()
            .map(|l| CodeContextItem {
                file_path: format!("src/main.{}", l),
                language: l.to_string(),
                relevance: 0.9,
                snippet: "placeholder".to_string(),
            })
            .collect()
    }

    fn make_intent(message: &str) -> HydrationIntent {
        HydrationIntent {
            message: message.to_string(),
            repo: None,
            file_paths: vec![],
            ticket_refs: vec![],
            urls: vec![],
            service_names: vec![],
        }
    }

    // ─── Prompt formatting ───────────────────────────────────────────────────

    #[test]
    fn test_format_empty_context() {
        let context = HydratedContext {
            code_context: vec![],
            doc_context: vec![],
            ownership: vec![],
            test_context: vec![],
            metadata: serde_json::Value::Object(Default::default()),
            recommended_tools: vec![],
        };
        let prompt = ContextHydrator::format_as_prompt(&context);
        assert!(prompt.is_empty(), "Empty context should produce empty prompt");
    }

    #[test]
    fn test_format_code_context_section() {
        let context = HydratedContext {
            code_context: vec![CodeContextItem {
                file_path: "src/payments/gateway.java".to_string(),
                language: "java".to_string(),
                relevance: 0.95,
                snippet: "public class PaymentGateway {".to_string(),
            }],
            doc_context: vec![],
            ownership: vec![],
            test_context: vec![],
            metadata: serde_json::Value::Object(Default::default()),
            recommended_tools: vec![],
        };
        let prompt = ContextHydrator::format_as_prompt(&context);
        assert!(prompt.contains("Relevant Code Files"), "Should have code section header");
        assert!(prompt.contains("gateway.java"), "Should mention the file");
        assert!(prompt.contains("java"), "Should mention the language");
        assert!(prompt.contains("0.95"), "Should show relevance score");
    }

    #[test]
    fn test_format_doc_context_section() {
        let context = HydratedContext {
            code_context: vec![],
            doc_context: vec![DocContextItem {
                title: "SWIFT Integration Guide".to_string(),
                source: "confluence".to_string(),
                relevance: 0.82,
                snippet: "To integrate SWIFT MT103 messages...".to_string(),
            }],
            ownership: vec![],
            test_context: vec![],
            metadata: serde_json::Value::Object(Default::default()),
            recommended_tools: vec![],
        };
        let prompt = ContextHydrator::format_as_prompt(&context);
        assert!(prompt.contains("Relevant Documentation"), "Should have docs section header");
        assert!(prompt.contains("SWIFT Integration Guide"), "Should mention doc title");
        assert!(prompt.contains("confluence"), "Should mention source");
    }

    #[test]
    fn test_format_ownership_section() {
        let context = HydratedContext {
            code_context: vec![],
            doc_context: vec![],
            ownership: vec![OwnershipItem {
                entity: "src/payments/gateway.java".to_string(),
                team: "Payments Platform".to_string(),
                slack_channel: Some("#payments-eng".to_string()),
            }],
            test_context: vec![],
            metadata: serde_json::Value::Object(Default::default()),
            recommended_tools: vec![],
        };
        let prompt = ContextHydrator::format_as_prompt(&context);
        assert!(prompt.contains("Code Ownership"), "Should have ownership section");
        assert!(prompt.contains("Payments Platform"), "Should mention team name");
        assert!(prompt.contains("#payments-eng"), "Should mention Slack channel");
    }

    #[test]
    fn test_format_test_context_section() {
        let context = HydratedContext {
            code_context: vec![],
            doc_context: vec![],
            ownership: vec![],
            test_context: vec![TestContextItem {
                test_file: "src/test/PaymentGatewayTest.java".to_string(),
                covers: "src/payments/gateway.java".to_string(),
            }],
            metadata: serde_json::Value::Object(Default::default()),
            recommended_tools: vec![],
        };
        let prompt = ContextHydrator::format_as_prompt(&context);
        assert!(prompt.contains("Related Tests"), "Should have tests section");
        assert!(prompt.contains("PaymentGatewayTest"), "Should mention test file");
    }

    #[test]
    fn test_format_all_sections_present() {
        let context = HydratedContext {
            code_context: vec![CodeContextItem {
                file_path: "src/foo.java".to_string(),
                language: "java".to_string(),
                relevance: 0.9,
                snippet: "class Foo".to_string(),
            }],
            doc_context: vec![DocContextItem {
                title: "Foo Guide".to_string(),
                source: "confluence".to_string(),
                relevance: 0.7,
                snippet: "How to use Foo".to_string(),
            }],
            ownership: vec![OwnershipItem {
                entity: "src/foo.java".to_string(),
                team: "Foo Team".to_string(),
                slack_channel: None,
            }],
            test_context: vec![TestContextItem {
                test_file: "src/FooTest.java".to_string(),
                covers: "src/foo.java".to_string(),
            }],
            metadata: serde_json::Value::Object(Default::default()),
            recommended_tools: vec!["developer__shell".to_string()],
        };
        let prompt = ContextHydrator::format_as_prompt(&context);
        assert!(prompt.contains("Relevant Code Files"));
        assert!(prompt.contains("Relevant Documentation"));
        assert!(prompt.contains("Code Ownership"));
        assert!(prompt.contains("Related Tests"));
    }

    // ─── HydrationIntent ─────────────────────────────────────────────────────

    #[test]
    fn test_intent_structure() {
        let intent = HydrationIntent {
            message: "Fix the payment gateway timeout issue, see CORE-1234".to_string(),
            repo: Some("payment-service".to_string()),
            file_paths: vec!["src/payments/gateway.java".to_string()],
            ticket_refs: vec!["CORE-1234".to_string()],
            urls: vec!["https://jira.example.com/CORE-1234".to_string()],
            service_names: vec!["payment-gateway".to_string()],
        };

        assert_eq!(intent.ticket_refs.len(), 1);
        assert_eq!(intent.file_paths.len(), 1);
        assert!(intent.repo.is_some());
    }
}
