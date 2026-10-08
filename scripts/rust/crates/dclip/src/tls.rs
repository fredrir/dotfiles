use std::path::{Path, PathBuf};
use std::sync::Arc;

use hostkit::Host;
use rustls::client::Resumption;
use rustls::crypto::{CryptoProvider, ring};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::{NoServerSessionStorage, WebPkiClientVerifier};
use rustls::{ClientConfig, RootCertStore, ServerConfig, ServerConnection};
use x509_cert::Certificate;
use x509_cert::der::Decode;

const PKI: &str = ".local/share/wezterm/mtls";

struct Pki {
    roots: RootCertStore,
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

fn load() -> Result<Pki, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    let directory = PathBuf::from(home).join(PKI);
    let certificates = |name: &str| -> Result<Vec<CertificateDer<'static>>, String> {
        let path = directory.join(name);
        CertificateDer::pem_file_iter(&path)
            .and_then(|iter| iter.collect::<Result<Vec<_>, _>>())
            .map_err(|error| unreadable(&path, error))
    };
    let mut roots = RootCertStore::empty();
    for ca in certificates("ca.pem")? {
        roots.add(ca).map_err(|error| format!("ca.pem: {error}"))?;
    }
    let key_path = directory.join("private_key.pem");
    let key =
        PrivateKeyDer::from_pem_file(&key_path).map_err(|error| unreadable(&key_path, error))?;
    Ok(Pki {
        roots,
        chain: certificates("cert.pem")?,
        key,
    })
}

fn unreadable(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{}: {error}", path.display())
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

pub fn client() -> Result<Arc<ClientConfig>, String> {
    let pki = load()?;
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .with_root_certificates(pki.roots)
        .with_client_auth_cert(pki.chain, pki.key)
        .map_err(|error| error.to_string())?;
    config.resumption = Resumption::disabled();
    config.enable_sni = false;
    Ok(Arc::new(config))
}

pub fn server() -> Result<Arc<ServerConfig>, String> {
    let pki = load()?;
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(pki.roots), provider())
        .build()
        .map_err(|error| error.to_string())?;
    let mut config = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .with_client_cert_verifier(verifier)
        .with_single_cert(pki.chain, pki.key)
        .map_err(|error| error.to_string())?;
    config.session_storage = Arc::new(NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    Ok(Arc::new(config))
}

pub fn server_name(host: Host) -> Result<ServerName<'static>, String> {
    ServerName::try_from(host.name()).map_err(|error| error.to_string())
}

pub fn common_name(der: &[u8]) -> Option<String> {
    Certificate::from_der(der)
        .ok()?
        .tbs_certificate()
        .subject()
        .common_name()
        .ok()
        .flatten()
        .map(String::from)
}

pub fn authorize(connection: &ServerConnection, user: &str) -> Result<(), String> {
    let name = connection
        .peer_certificates()
        .and_then(|chain| chain.first())
        .and_then(|leaf| common_name(leaf));
    match name {
        Some(name) if name == user => Ok(()),
        Some(name) => Err(format!("client CN={name} is not {user}")),
        None => Err("client certificate has no CN".into()),
    }
}

pub fn user() -> Result<String, String> {
    if let Some(user) = std::env::var_os("USER").filter(|user| !user.is_empty()) {
        return Ok(user.to_string_lossy().into_owned());
    }
    nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|user| user.name)
        .ok_or_else(|| "no user name".into())
}

#[cfg(test)]
#[path = "../tests/unit/tls_tests.rs"]
mod tests;
