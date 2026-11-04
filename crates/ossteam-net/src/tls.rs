// SPDX-License-Identifier: GPL-2.0-or-later
//! TLS configuration shared by every HTTPS and WSS connection (SEC-3).
//!
//! rustls with the ring provider and the operating system's certificate
//! verifier. The one way to weaken verification is
//! [`unverified_agent_config`], which SEC-3 allows only for a game's
//! `ISteamHTTP` request that asks for it.

use std::sync::{Arc, OnceLock};

use rustls::ClientConfig;
use rustls::crypto::CryptoProvider;
use rustls_platform_verifier::BuilderVerifierExt;

fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

/// The rustls client configuration for WSS connections.
pub fn client_config() -> Result<Arc<ClientConfig>, rustls::Error> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    if let Some(c) = CONFIG.get() {
        return Ok(c.clone());
    }
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_platform_verifier()?
        .with_no_client_auth();
    Ok(CONFIG.get_or_init(|| Arc::new(config)).clone())
}

/// An agent configuration with the TLS policy set, for callers that need
/// their own timeouts or pool sizes.
pub fn agent_config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    ureq::Agent::config_builder().tls_config(
        TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .unversioned_rustls_crypto_provider(provider())
            .root_certs(RootCerts::PlatformVerifier)
            .build(),
    )
}

/// An agent configuration that does not check certificates: only for a
/// game's `ISteamHTTP` request after
/// `SetHTTPRequestRequiresVerifiedCertificate(false)` (SEC-3,
/// `isteamhttp.md` §4).
pub fn unverified_agent_config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    use ureq::tls::{TlsConfig, TlsProvider};
    ureq::Agent::config_builder().tls_config(
        TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .unversioned_rustls_crypto_provider(provider())
            .disable_verification(true)
            .build(),
    )
}

/// An HTTP agent for WebAPI requests, with the same TLS policy.
pub fn http_agent() -> ureq::Agent {
    agent_config()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .new_agent()
}
