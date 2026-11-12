//! 1-Click WhatsApp Business Cloud API payload translator.
//!
//! Converts legacy Meta Graph API template requests (`WhatsAppTemplateRequest`)
//! into native declarative `SonaraimCardDraft` structures.

use crate::consent::MessageCategory;
use std::fmt;

/// Incoming request shaped after the WhatsApp Business Cloud API template message payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhatsAppTemplateRequest {
    /// Messaging product identifier (expected to be `"whatsapp"`).
    pub messaging_product: String,
    /// Recipient identifier (phone number or Sonaraim DID).
    pub to: String,
    /// Name of the pre-registered WhatsApp template (e.g., `"order_confirmation"`).
    pub template_name: String,
    /// BCP-47 / locale code (e.g., `"en_US"` or `"es_PE"`).
    pub language_code: String,
    /// Template category (`"TRANSACTIONAL"`, `"UTILITY"`, `"AUTHENTICATION"`, or `"MARKETING"`).
    pub category: String,
    /// Ordered template body variable substitutions.
    pub body_parameters: Vec<String>,
    /// Interactive button labels attached to the template.
    pub buttons: Vec<String>,
}

/// Declarative Sonaraim interactive card draft produced from a legacy webhook payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SonaraimCardDraft {
    /// Target recipient DID or address.
    pub recipient: String,
    /// Card header title synthesized from the template name and locale.
    pub title: String,
    /// Rendered card body combining template parameters.
    pub body: String,
    /// Interactive action buttons rendered on the native card.
    pub actions: Vec<String>,
    /// Mapped Sonaraim consent category (`Transactional` vs `Promotional`).
    pub category: MessageCategory,
}

impl SonaraimCardDraft {
    /// Formats the card draft as a JSON payload string.
    pub fn to_json(&self) -> String {
        let cat = match self.category {
            MessageCategory::Transactional => "TRANSACTIONAL",
            MessageCategory::Promotional => "PROMOTIONAL",
        };
        format!(
            r#"{{"recipient":"{}","title":"{}","body":"{}","category":"{cat}"}}"#,
            self.recipient, self.title, self.body
        )
    }
}

/// Errors returned when translating a legacy WhatsApp Cloud API request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    /// The `messaging_product` field is not `"whatsapp"`.
    UnsupportedMessagingProduct(String),
    /// A required field (`to` or `template_name`) is empty.
    MissingField(&'static str),
    /// The template category is unrecognized.
    UnknownCategory(String),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMessagingProduct(prod) => {
                write!(
                    f,
                    "unsupported messaging_product '{prod}', expected 'whatsapp'"
                )
            }
            Self::MissingField(field) => write!(f, "missing required WhatsApp field: {field}"),
            Self::UnknownCategory(cat) => write!(f, "unknown WhatsApp template category: {cat}"),
        }
    }
}

impl std::error::Error for AdapterError {}

/// Stateless 1-Click adapter translating WhatsApp Cloud API requests into `SonaraimCardDraft`.
#[derive(Debug, Default, Clone, Copy)]
pub struct WhatsAppCloudAdapter;

impl WhatsAppCloudAdapter {
    /// Translates a [`WhatsAppTemplateRequest`] into a native [`SonaraimCardDraft`].
    ///
    /// # Category Mapping
    /// - `"TRANSACTIONAL"`, `"UTILITY"`, and `"AUTHENTICATION"` (case-insensitive) map to
    ///   [`MessageCategory::Transactional`].
    /// - `"MARKETING"` and `"PROMOTIONAL"` (case-insensitive) map to
    ///   [`MessageCategory::Promotional`].
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError`] if the product is not `"whatsapp"`, required fields are blank,
    /// or the category string is unknown.
    pub fn translate(req: &WhatsAppTemplateRequest) -> Result<SonaraimCardDraft, AdapterError> {
        if !req.messaging_product.eq_ignore_ascii_case("whatsapp") {
            return Err(AdapterError::UnsupportedMessagingProduct(
                req.messaging_product.clone(),
            ));
        }
        if req.to.trim().is_empty() {
            return Err(AdapterError::MissingField("to"));
        }
        if req.template_name.trim().is_empty() {
            return Err(AdapterError::MissingField("template_name"));
        }

        let category = match req.category.trim().to_ascii_uppercase().as_str() {
            "TRANSACTIONAL" | "UTILITY" | "AUTHENTICATION" => MessageCategory::Transactional,
            "MARKETING" | "PROMOTIONAL" => MessageCategory::Promotional,
            other => return Err(AdapterError::UnknownCategory(other.to_string())),
        };

        let locale = if req.language_code.trim().is_empty() {
            "en_US"
        } else {
            req.language_code.trim()
        };

        let title = format!("template:{} ({})", req.template_name.trim(), locale);
        let body = if req.body_parameters.is_empty() {
            format!("[{}]", req.template_name.trim())
        } else {
            req.body_parameters.join(" | ")
        };

        Ok(SonaraimCardDraft {
            recipient: req.to.trim().to_string(),
            title,
            body,
            actions: req.buttons.clone(),
            category,
        })
    }
}
