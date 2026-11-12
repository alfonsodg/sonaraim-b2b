//! Cryptographic Consent Gate enforcing `did:web` verification and separation
//! between transactional notifications and promotional marketing messages.

use std::fmt;

/// Classification of an enterprise-to-consumer message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageCategory {
    /// Essential service notifications (OTPs, receipts, fraud alerts, shipping updates).
    /// Allowed without a promotional token for verified `did:web` tenants.
    Transactional,
    /// Marketing, upsell, or promotional campaigns.
    /// Strictly requires an active, unrevoked user `ConsentToken`.
    Promotional,
}

/// User-signed cryptographic opt-in token granting a specific enterprise permission
/// to dispatch promotional messages until `expires_at_unix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentToken {
    /// Unique identifier of the consent grant.
    pub token_id: String,
    /// Enterprise `did:web` identifier authorized by the user.
    pub tenant_did: String,
    /// Recipient user DID who granted the opt-in consent.
    pub recipient_did: String,
    /// Expiration timestamp in Unix seconds.
    pub expires_at_unix: u64,
    /// Whether the user has revoked this consent token.
    pub revoked: bool,
    /// Cryptographic signature hex/base64 proof attached to the grant.
    pub signature: String,
}

/// Errors raised when `ConsentGate` rejects a dispatch request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentError {
    /// The enterprise tenant does not hold a verified `did:web:` identity.
    UnverifiedTenantDid(String),
    /// A promotional message was submitted without a `ConsentToken`.
    MissingConsentToken,
    /// The provided `ConsentToken` was revoked by the recipient.
    RevokedConsentToken(String),
    /// The provided `ConsentToken` has expired at the current Unix timestamp.
    ExpiredConsentToken {
        /// Token identifier.
        token_id: String,
        /// Expiration timestamp in Unix seconds.
        expires_at_unix: u64,
    },
    /// The `ConsentToken` belongs to a different tenant or recipient DID, or has an empty signature.
    InvalidConsentScope(String),
}

impl fmt::Display for ConsentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnverifiedTenantDid(did) => {
                write!(f, "tenant DID '{did}' is not a verified did:web identity")
            }
            Self::MissingConsentToken => {
                write!(f, "promotional messages require a valid ConsentToken")
            }
            Self::RevokedConsentToken(id) => write!(f, "consent token '{id}' has been revoked"),
            Self::ExpiredConsentToken {
                token_id,
                expires_at_unix,
            } => write!(
                f,
                "consent token '{token_id}' expired at unix timestamp {expires_at_unix}"
            ),
            Self::InvalidConsentScope(reason) => write!(f, "invalid consent scope: {reason}"),
        }
    }
}

impl std::error::Error for ConsentError {}

/// Decision summary returned when `ConsentGate` authorizes a message dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchAuthorization {
    /// Verified enterprise `did:web` identifier.
    pub tenant_did: String,
    /// Target recipient identifier.
    pub recipient_did: String,
    /// Authorized message category.
    pub category: MessageCategory,
    /// Optional verified consent token ID (populated for promotional messages).
    pub verified_token_id: Option<String>,
}

/// Protocol-level gate verifying enterprise identity and recipient consent before routing.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConsentGate;

impl ConsentGate {
    /// Authorizes or rejects an enterprise message dispatch.
    ///
    /// # Rules
    /// 1. `tenant_did` must start with `"did:web:"` and `did_verified` must be `true`.
    /// 2. `MessageCategory::Transactional` is immediately authorized for verified `did:web` tenants.
    /// 3. `MessageCategory::Promotional` requires a `ConsentToken` whose `tenant_did` and
    ///    `recipient_did` match the request, whose `signature` is non-empty, which is not
    ///    `revoked`, and whose `expires_at_unix > now_unix_secs`.
    ///
    /// # Errors
    ///
    /// Returns [`ConsentError`] when any identity or consent invariant fails.
    pub fn authorize_dispatch(
        tenant_did: &str,
        did_verified: bool,
        recipient_did: &str,
        category: MessageCategory,
        consent_token: Option<&ConsentToken>,
        now_unix_secs: u64,
    ) -> Result<DispatchAuthorization, ConsentError> {
        if !did_verified || !tenant_did.starts_with("did:web:") || tenant_did.len() <= 8 {
            return Err(ConsentError::UnverifiedTenantDid(tenant_did.to_string()));
        }

        match category {
            MessageCategory::Transactional => Ok(DispatchAuthorization {
                tenant_did: tenant_did.to_string(),
                recipient_did: recipient_did.to_string(),
                category,
                verified_token_id: None,
            }),
            MessageCategory::Promotional => {
                let token = consent_token.ok_or(ConsentError::MissingConsentToken)?;

                if token.revoked {
                    return Err(ConsentError::RevokedConsentToken(token.token_id.clone()));
                }
                if now_unix_secs >= token.expires_at_unix {
                    return Err(ConsentError::ExpiredConsentToken {
                        token_id: token.token_id.clone(),
                        expires_at_unix: token.expires_at_unix,
                    });
                }
                if token.tenant_did != tenant_did {
                    return Err(ConsentError::InvalidConsentScope(
                        "token tenant_did does not match sender".to_string(),
                    ));
                }
                if token.recipient_did != recipient_did {
                    return Err(ConsentError::InvalidConsentScope(
                        "token recipient_did does not match recipient".to_string(),
                    ));
                }
                if token.signature.trim().is_empty() {
                    return Err(ConsentError::InvalidConsentScope(
                        "token cryptographic signature is empty".to_string(),
                    ));
                }

                Ok(DispatchAuthorization {
                    tenant_did: tenant_did.to_string(),
                    recipient_did: recipient_did.to_string(),
                    category,
                    verified_token_id: Some(token.token_id.clone()),
                })
            }
        }
    }
}
