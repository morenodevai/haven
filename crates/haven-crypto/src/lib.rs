/// Haven Crypto Library
///
/// Phase 0 MVP: Shared symmetric key encryption (AES-256-GCM).
/// All users in a channel share the same key (distributed out-of-band for now).
///
/// Future phases will replace this with:
/// - Signal Protocol (X3DH + Double Ratchet) for DMs
/// - MLS (RFC 9420) for group channels
/// - SFrame for voice/video E2EE

#[cfg(feature = "client")]
pub mod encrypt;

pub mod keys;
