//! SaaS subscription tiers and 0%-markup payment take-rate billing helpers.

use std::fmt;

/// Supported regional and global payment rails for B2B conversational commerce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentProvider {
    /// Global card and ACH processing via Stripe.
    Stripe,
    /// Latin America checkout rail via Mercado Pago.
    MercadoPago,
    /// Peruvian instant QR/phone rail via Yape / Plin.
    YapePlin,
    /// On-chain USDC settlement on low-fee L2 networks.
    StablecoinUsdc,
}

impl PaymentProvider {
    /// Returns the canonical lowercase identifier for the payment provider.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stripe => "stripe",
            Self::MercadoPago => "mercado_pago",
            Self::YapePlin => "yape_plin",
            Self::StablecoinUsdc => "stablecoin_usdc",
        }
    }

    /// Parses a payment provider name (case-insensitive).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stripe" => Some(Self::Stripe),
            "mercado_pago" | "mercadopago" => Some(Self::MercadoPago),
            "yape_plin" | "yapeplin" | "yape" | "plin" => Some(Self::YapePlin),
            "stablecoin_usdc" | "usdc" => Some(Self::StablecoinUsdc),
            _ => None,
        }
    }
}

/// B2B SaaS subscription tiers with zero per-message markup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaasPlanTier {
    /// Developer sandbox and low-volume starter tier.
    Free,
    /// Growth tier for small and medium businesses.
    SmbPro,
    /// Dedicated sovereign cluster tier for enterprise deployments.
    EnterpriseSovereign,
}

impl SaasPlanTier {
    /// Returns the canonical plan tier identifier.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::SmbPro => "smb_pro",
            Self::EnterpriseSovereign => "enterprise_sovereign",
        }
    }
}

/// Errors returned during billing and take-rate calculations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BillingError {
    /// Payment intent amount must be strictly positive.
    InvalidAmount,
    /// Take-rate basis points exceed 10,000 (100%).
    InvalidTakeRateBps,
}

impl fmt::Display for BillingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAmount => write!(f, "payment amount must be greater than zero"),
            Self::InvalidTakeRateBps => write!(f, "take_rate_bps cannot exceed 10000"),
        }
    }
}

impl std::error::Error for BillingError {}

/// Computes the platform take-rate fee and merchant net settlement in minor units.
///
/// Uses 128-bit intermediate arithmetic to prevent overflow on large settlements.
pub fn compute_take_rate_minor_units(
    amount_minor: u64,
    take_rate_bps: u64,
) -> Result<(u64, u64), BillingError> {
    if amount_minor == 0 {
        return Err(BillingError::InvalidAmount);
    }
    if take_rate_bps > 10_000 {
        return Err(BillingError::InvalidTakeRateBps);
    }
    let fee_u128 = (u128::from(amount_minor) * u128::from(take_rate_bps)) / 10_000;
    let fee_minor = fee_u128 as u64;
    let net_minor = amount_minor.saturating_sub(fee_minor);
    Ok((fee_minor, net_minor))
}
