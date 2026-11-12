//! CRM and helpdesk event normalizers for omnichannel B2B ingestion.

use std::fmt;

/// Supported external CRM and helpdesk platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrmProvider {
    /// Open-source Chatwoot customer engagement suite.
    Chatwoot,
    /// Zendesk Support ticketing & messaging.
    Zendesk,
    /// HubSpot CRM conversations.
    HubSpot,
}

impl CrmProvider {
    /// Returns the canonical lowercase provider identifier.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chatwoot => "chatwoot",
            Self::Zendesk => "zendesk",
            Self::HubSpot => "hubspot",
        }
    }

    /// Parses a CRM provider name (case-insensitive).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "chatwoot" => Some(Self::Chatwoot),
            "zendesk" => Some(Self::Zendesk),
            "hubspot" => Some(Self::HubSpot),
            _ => None,
        }
    }
}

/// Unified internal event generated from heterogeneous CRM webhooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedCrmEvent {
    /// Source CRM provider identifier.
    pub provider: String,
    /// External conversation or ticket identifier.
    pub conversation_id: String,
    /// Sanitized message text to dispatch as a Sonaraim card.
    pub text: String,
}

/// Errors returned when normalizing an external CRM webhook event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrmError {
    /// External conversation identifier is blank.
    EmptyConversationId,
    /// Message text is blank.
    EmptyText,
}

impl fmt::Display for CrmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyConversationId => write!(f, "conversation_id cannot be empty"),
            Self::EmptyText => write!(f, "CRM event text cannot be empty"),
        }
    }
}

impl std::error::Error for CrmError {}

/// Validates and normalizes an incoming CRM webhook event into [`NormalizedCrmEvent`].
pub fn normalize_crm_event(
    provider: CrmProvider,
    conversation_id: &str,
    text: &str,
) -> Result<NormalizedCrmEvent, CrmError> {
    let trimmed_id = conversation_id.trim();
    if trimmed_id.is_empty() {
        return Err(CrmError::EmptyConversationId);
    }
    let trimmed_text = text.trim();
    if trimmed_text.is_empty() {
        return Err(CrmError::EmptyText);
    }
    Ok(NormalizedCrmEvent {
        provider: provider.as_str().to_string(),
        conversation_id: trimmed_id.to_string(),
        text: trimmed_text.to_string(),
    })
}
