//! Sonaraim Enterprise B2B Bridge library crate.
//!
//! Exports the zero-hardcode configuration loader, cryptographic Consent Gate,
//! 1-Click WhatsApp Business Cloud API template adapter, and Smart Fallback
//! routing engine.

pub mod billing;
pub mod config;
pub mod consent;
pub mod crm_connectors;
pub mod fallback;
pub mod hmac_sha256;
pub mod http_api;
pub mod outbound_dispatcher;
pub mod persistence;
pub mod routes;
pub mod url_validator;
pub mod webhooks;
pub mod whatsapp_adapter;
