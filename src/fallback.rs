//! Smart Fallback Engine routing B2B cards over native MLS E2EE when the recipient
//! is registered, or generating an Instant Web / App Clip magic link otherwise.

use std::fmt;

/// Resolved delivery route produced by [`SmartFallbackEngine`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryRoute {
    /// Recipient is onboarded on Sonaraim and reachable via native MLS E2EE on `sonaraim-node`.
    NativeMlsE2ee {
        /// Upstream `sonaraim-node` RPC URL.
        node_rpc_url: String,
        /// Target recipient DID.
        recipient_did: String,
    },
    /// Recipient is not yet onboarded; deliver via a signed Instant Web / App Clip magic link.
    InstantClip {
        /// Full magic link URL (`{B2B_INSTANT_CLIP_BASE_URL}/c/{clip_token}`).
        magic_link_url: String,
        /// Ephemeral signed token identifying the encrypted card session.
        clip_token: String,
    },
}

/// Errors returned by [`SmartFallbackEngine`] when resolving a delivery route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackError {
    /// Recipient identifier is empty.
    EmptyRecipient,
    /// Ephemeral clip token is empty when attempting to build an Instant Clip link.
    EmptyClipToken,
}

impl fmt::Display for FallbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRecipient => write!(f, "recipient identifier cannot be empty"),
            Self::EmptyClipToken => write!(f, "ephemeral clip token cannot be empty"),
        }
    }
}

impl std::error::Error for FallbackError {}

/// Engine that selects between native MLS E2EE delivery and Instant Web / App Clip magic links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartFallbackEngine {
    node_rpc_url: String,
    instant_clip_base_url: String,
}

impl SmartFallbackEngine {
    /// Creates a new [`SmartFallbackEngine`] from configured upstream URLs.
    pub fn new(node_rpc_url: impl Into<String>, instant_clip_base_url: impl Into<String>) -> Self {
        let base = instant_clip_base_url.into();
        Self {
            node_rpc_url: node_rpc_url.into(),
            instant_clip_base_url: base.trim_end_matches('/').to_string(),
        }
    }

    /// Resolves the optimal delivery route for a recipient.
    ///
    /// # Parameters
    /// - `recipient`: Recipient DID or phone identifier.
    /// - `recipient_onboarded`: `true` if the recipient has published MLS KeyPackages on Sonaraim.
    /// - `clip_token`: Ephemeral token used to construct `{B2B_INSTANT_CLIP_BASE_URL}/c/{token}`
    ///   when `recipient_onboarded` is `false`.
    ///
    /// # Errors
    ///
    /// Returns [`FallbackError`] if `recipient` is blank, or if `clip_token` is blank when
    /// fallback link generation is required.
    pub fn resolve_route(
        &self,
        recipient: &str,
        recipient_onboarded: bool,
        clip_token: &str,
    ) -> Result<DeliveryRoute, FallbackError> {
        let trimmed_recipient = recipient.trim();
        if trimmed_recipient.is_empty() {
            return Err(FallbackError::EmptyRecipient);
        }

        if recipient_onboarded {
            Ok(DeliveryRoute::NativeMlsE2ee {
                node_rpc_url: self.node_rpc_url.clone(),
                recipient_did: trimmed_recipient.to_string(),
            })
        } else {
            let trimmed_token = clip_token.trim();
            if trimmed_token.is_empty() {
                return Err(FallbackError::EmptyClipToken);
            }
            let magic_link_url = format!("{}/c/{}", self.instant_clip_base_url, trimmed_token);
            Ok(DeliveryRoute::InstantClip {
                magic_link_url,
                clip_token: trimmed_token.to_string(),
            })
        }
    }
}
