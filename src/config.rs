//! Zero-hardcode runtime configuration loader for `sonaraim-b2b`.

use std::env;
use std::fmt;

/// Errors returned when loading or validating `B2bConfig` from environment variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A required environment variable is missing or empty.
    MissingVar(&'static str),
    /// An environment variable failed numeric or structural parsing.
    InvalidVar {
        /// Name of the invalid environment variable.
        name: &'static str,
        /// Explanation of why the value is invalid.
        reason: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingVar(name) => write!(f, "missing required environment variable: {name}"),
            Self::InvalidVar { name, reason } => {
                write!(f, "invalid environment variable {name}: {reason}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Runtime configuration for the Sonaraim B2B Enterprise Bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct B2bConfig {
    /// Bind host for the HTTP REST and WhatsApp adapter server (`B2B_HTTP_HOST`).
    pub http_host: String,
    /// Bind port for the HTTP REST and WhatsApp adapter server (`B2B_HTTP_PORT`).
    pub http_port: u16,
    /// Bind port for the internal gRPC enterprise bridge (`B2B_GRPC_PORT`).
    pub grpc_port: u16,
    /// Upstream `sonaraim-node` RPC URL (`B2B_NODE_RPC_URL`).
    pub node_rpc_url: String,
    /// Base URL for Instant Web / App Clip magic links (`B2B_INSTANT_CLIP_BASE_URL`).
    pub instant_clip_base_url: String,
    /// Maximum promotional requests per second per tenant (`B2B_PROMO_RATE_LIMIT_RPS`).
    pub promo_rate_limit_rps: u32,
    /// File path for durable JSONL state snapshots (`B2B_STORAGE_PATH`).
    pub storage_path: String,
    /// Whether `http://` and loopback callback URLs are permitted (dev/test only via `B2B_ALLOW_INSECURE_CALLBACKS`).
    pub allow_insecure_callbacks: bool,
}

impl B2bConfig {
    /// Loads `B2bConfig` strictly from process environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if any required variable is missing, empty, or invalid.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| env::var(key).ok())
    }

    /// Loads `B2bConfig` using a custom lookup closure for deterministic testing.
    ///
    /// # Parameters
    /// - `lookup`: Function mapping an environment variable name to `Option<String>`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if any required variable is missing, empty, or invalid.
    pub fn from_lookup<F>(mut lookup: F) -> Result<Self, ConfigError>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let http_host = Self::require_non_empty(&mut lookup, "B2B_HTTP_HOST")?;
        let http_port = Self::require_u16(&mut lookup, "B2B_HTTP_PORT")?;
        let grpc_port = Self::require_u16(&mut lookup, "B2B_GRPC_PORT")?;
        let node_rpc_url = Self::require_non_empty(&mut lookup, "B2B_NODE_RPC_URL")?;
        let instant_clip_base_url =
            Self::require_non_empty(&mut lookup, "B2B_INSTANT_CLIP_BASE_URL")?
                .trim_end_matches('/')
                .to_string();
        let promo_rate_limit_rps = Self::require_u32(&mut lookup, "B2B_PROMO_RATE_LIMIT_RPS")?;
        let storage_path = Self::require_non_empty(&mut lookup, "B2B_STORAGE_PATH")?;
        let allow_insecure_callbacks =
            Self::optional_bool(&mut lookup, "B2B_ALLOW_INSECURE_CALLBACKS")?;

        Ok(Self {
            http_host,
            http_port,
            grpc_port,
            node_rpc_url,
            instant_clip_base_url,
            promo_rate_limit_rps,
            storage_path,
            allow_insecure_callbacks,
        })
    }

    /// Reads a required non-empty string variable using the provided lookup closure.
    fn require_non_empty<F>(lookup: &mut F, name: &'static str) -> Result<String, ConfigError>
    where
        F: FnMut(&str) -> Option<String>,
    {
        match lookup(name) {
            Some(val) if !val.trim().is_empty() => Ok(val.trim().to_string()),
            _ => Err(ConfigError::MissingVar(name)),
        }
    }

    /// Reads and parses a required non-zero `u16` port variable.
    fn require_u16<F>(lookup: &mut F, name: &'static str) -> Result<u16, ConfigError>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let raw = Self::require_non_empty(lookup, name)?;
        let parsed = raw.parse::<u16>().map_err(|_| ConfigError::InvalidVar {
            name,
            reason: format!("expected u16 port, got '{raw}'"),
        })?;
        if parsed == 0 {
            return Err(ConfigError::InvalidVar {
                name,
                reason: "port cannot be 0".to_string(),
            });
        }
        Ok(parsed)
    }

    /// Reads and parses a required non-zero `u32` integer variable.
    fn require_u32<F>(lookup: &mut F, name: &'static str) -> Result<u32, ConfigError>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let raw = Self::require_non_empty(lookup, name)?;
        let parsed = raw.parse::<u32>().map_err(|_| ConfigError::InvalidVar {
            name,
            reason: format!("expected positive u32, got '{raw}'"),
        })?;
        if parsed == 0 {
            return Err(ConfigError::InvalidVar {
                name,
                reason: "rate limit cannot be 0".to_string(),
            });
        }
        Ok(parsed)
    }

    /// Reads an optional boolean variable, defaulting to `false` when unset or empty.
    fn optional_bool<F>(lookup: &mut F, name: &'static str) -> Result<bool, ConfigError>
    where
        F: FnMut(&str) -> Option<String>,
    {
        match lookup(name) {
            None => Ok(false),
            Some(val) if val.trim().is_empty() => Ok(false),
            Some(val) => match val.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Ok(true),
                "false" | "0" | "no" => Ok(false),
                _ => Err(ConfigError::InvalidVar {
                    name,
                    reason: format!("expected boolean, got '{}'", val.trim()),
                }),
            },
        }
    }
}
