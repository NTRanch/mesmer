//! mesmer — a deterministic, consent-gated hypnosis content
//! composer (Nice Thoughts Ranch). The lib carries the real
//! surface; the binary is a thin MCP shell around it.
//!
//! Built so far:
//! - R1 — the canon as validated data ([`canon`]) and the consent
//!   contracts ([`consent`]): `ConsentProfile`, `Envelope`,
//!   `EffectClass`, `SessionRequest`, `ProgrammeGrant`.
//! - R2 — the pure consent gate ([`gate::decide`] and the
//!   lifecycle gates), persistence with the disclosure seam
//!   ([`store`]), and the six consent tools ([`tools`]).
//!
//! Not built (out of scope until commissioned): R3 composition,
//! R4 programmes and the full disclosure log, R5 consumers.

#![forbid(unsafe_code)]

pub mod canon;
pub mod consent;
pub mod error;
pub mod gate;
pub mod guard;
pub mod server;
pub mod store;
pub mod tools;
