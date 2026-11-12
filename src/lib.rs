//! Sonaraim Enterprise B2B Bridge library crate.
//!
//! Exports the zero-hardcode configuration loader, cryptographic Consent Gate,
//! 1-Click WhatsApp Business Cloud API template adapter, and Smart Fallback
//! routing engine.

pub mod config;
pub mod consent;
pub mod fallback;
pub mod whatsapp_adapter;
