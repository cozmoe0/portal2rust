//! HTTPS: browsers only offer WebGPU to secure pages, and a page from
//! another machine on the network is only secure over HTTPS. Without a
//! certificate of the user's own (`--cert`/`--key`, for example one made
//! with mkcert), a self-signed one is made for this machine's name and
//! addresses and kept in the data folder, so each browser accepts it once.

use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::Options;

/// This machine's IPv4 addresses other than loopback.
pub fn local_ipv4() -> Vec<IpAddr> {
    let mut ips: Vec<IpAddr> =
        if_addrs::get_if_addrs().unwrap_or_default().into_iter().map(|i| i.ip()).filter(|ip| ip.is_ipv4() && !ip.is_loopback()).collect();
    ips.sort();
    ips.dedup();
    ips
}

fn host_name() -> Option<String> {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok().map(|h| h.to_ascii_lowercase()).filter(|h| !h.is_empty())
}

/// The names the certificate is for: localhost, this machine's name and
/// every address it has.
fn subject_names() -> Vec<String> {
    let mut names = vec!["localhost".to_owned(), "127.0.0.1".to_owned()];
    if let Some(host) = host_name() {
        names.push(host.clone());
        names.push(format!("{host}.local"));
    }
    names.extend(local_ipv4().into_iter().map(|ip| ip.to_string()));
    names
}

pub fn acceptor(opts: &Options, data_dir: &Path) -> Result<TlsAcceptor, String> {
    let (cert_pem, key_pem) = match (&opts.cert, &opts.key) {
        (Some(c), Some(k)) => {
            (std::fs::read(c).map_err(|e| format!("{}: {e}", c.display()))?, std::fs::read(k).map_err(|e| format!("{}: {e}", k.display()))?)
        }
        (None, None) => self_signed(data_dir)?,
        _ => return Err("--cert and --key go together".into()),
    };
    let certs: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut cert_pem.as_slice()).collect::<Result<_, _>>().map_err(|e| format!("certificate: {e}"))?;
    let key: PrivateKeyDer<'static> =
        rustls_pemfile::private_key(&mut key_pem.as_slice()).map_err(|e| format!("key: {e}"))?.ok_or("no private key in the key file")?;
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("certificate: {e}"))?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// The kept self-signed certificate, made again when this machine's names
/// or addresses have changed.
fn self_signed(data_dir: &Path) -> Result<(Vec<u8>, Vec<u8>), String> {
    let dir = data_dir.join("tls");
    let (cert_path, key_path, names_path) = (dir.join("cert.pem"), dir.join("key.pem"), dir.join("names.txt"));
    let names = subject_names();
    let joined = names.join("\n");
    if let (Ok(c), Ok(k), Ok(n)) = (std::fs::read(&cert_path), std::fs::read(&key_path), std::fs::read_to_string(&names_path))
        && n == joined
    {
        return Ok((c, k));
    }
    let made = rcgen::generate_simple_self_signed(names.clone()).map_err(|e| format!("certificate: {e}"))?;
    let (cert, key) = (made.cert.pem(), made.signing_key.serialize_pem());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(&cert_path, &cert).map_err(|e| e.to_string())?;
    std::fs::write(&key_path, &key).map_err(|e| e.to_string())?;
    std::fs::write(&names_path, &joined).map_err(|e| e.to_string())?;
    println!("p2web: made a self-signed certificate for {} ({})", names.join(", "), cert_path.display());
    Ok((cert.into_bytes(), key.into_bytes()))
}
