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
}
