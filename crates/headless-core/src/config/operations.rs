//! Operation models for rule and provider management.
use serde::{Deserialize, Serialize};

/// Action to perform on a provider resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAction {
    Update,
    Healthcheck,
}

/// Result summary of a provider operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOperationReceipt {
    pub name: String,
    pub action: ProviderAction,
    pub success: bool,
}

/// Parameters for delay testing a proxy or proxy group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DelayTestQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_operation_receipt_roundtrip() {
        let receipt = ProviderOperationReceipt {
            name: "test-provider".into(),
            action: ProviderAction::Update,
            success: true,
        };
        let json = serde_json::to_string(&receipt).expect("serialize");
        assert!(json.contains("\"action\":\"update\""));
        let decoded: ProviderOperationReceipt = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, receipt);
    }

    #[test]
    fn delay_test_query_roundtrip() {
        let query = DelayTestQuery {
            url: Some("http://www.gstatic.com/generate_204".into()),
            timeout: Some(5000),
        };
        let json = serde_json::to_string(&query).expect("serialize");
        assert!(json.contains("\"timeout\":5000"));
        let decoded: DelayTestQuery = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, query);

        let empty: DelayTestQuery = serde_json::from_str("{}").expect("deserialize empty");
        assert_eq!(empty.url, None);
        assert_eq!(empty.timeout, None);
    }
}
