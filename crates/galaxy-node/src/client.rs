use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::Arc,
    time::Duration,
};

use reqwest::dns::{Name, Resolve, Resolving};
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::{Host, Url};

use crate::{Error, Result};

pub(crate) const MAX_BODY: usize = 256 * 1024;
const MAX_URL: usize = 512;
const TIMEOUT: Duration = Duration::from_secs(4);

fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => public_v6(ip),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    !matches!(
        ip.octets(),
        [0 | 10 | 127 | 224..=255, ..]
            | [100, 64..=127, ..]
            | [169, 254, ..]
            | [172, 16..=31, ..]
            | [192, 0, 0 | 2, _]
            | [192, 88, 99, _]
            | [192, 168, ..]
            | [198, 18 | 19, ..]
            | [198, 51, 100, _]
            | [203, 0, 113, _]
    )
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    // Global unicast only. Exclude special-use assignments, documentation
    // addresses, and transition mechanisms inside that space.
    matches!(segments[0], 0x2000..=0x3fff)
        && !matches!(
            segments,
            [0x2001, 0..=0x1ff | 0xdb8, ..] | [0x2002 | 0x3ffe | 0x3fff, ..]
        )
}

pub(crate) fn base_url(text: &str, allow_private: bool) -> Result<Url> {
    if text.len() > MAX_URL || !text.bytes().all(|b| b.is_ascii_graphic()) || text.contains('\\') {
        return Err(Error::Peer("invalid peer URL"));
    }
    let mut url = Url::parse(text).map_err(|_| Error::Peer("invalid peer URL"))?;
    if (url.scheme() != "https" && !(allow_private && url.scheme() == "http"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
        || url.path() != "/"
    {
        return Err(Error::Peer(
            "peer URL must be an HTTPS origin (HTTP only when private peers are enabled)",
        ));
    }
    match url.host().ok_or(Error::Peer("missing peer host"))? {
        Host::Ipv4(ip) if !allow_private && !public(ip.into()) => {
            return Err(Error::Peer("non-public peer address"))
        }
        Host::Ipv6(ip) if !allow_private && !public(ip.into()) => {
            return Err(Error::Peer("non-public peer address"))
        }
        Host::Domain(name) if !allow_private => {
            let name = name.trim_end_matches('.');
            if !name.contains('.') || name.ends_with(".localhost") || name.ends_with(".local") {
                return Err(Error::Peer("non-public peer hostname"));
            }
        }
        _ => {}
    }
    url.set_path("/");
    Ok(url)
}

struct GuardedDns {
    allow_private: bool,
}

impl Resolve for GuardedDns {
    fn resolve(&self, name: Name) -> Resolving {
        let allow_private = self.allow_private;
        Box::pin(async move {
            // The actual socket connector receives only this checked DNS
            // result; there is no second DNS lookup between check and use.
            let addresses: Vec<_> = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .take(33)
                .collect();
            if addresses.is_empty()
                || addresses.len() > 32
                || addresses
                    .iter()
                    .any(|addr| !allow_private && !public(addr.ip()))
            {
                return Err(io::Error::other("peer DNS address not allowed").into());
            }
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

pub(crate) struct Client {
    inner: reqwest::Client,
}

impl Client {
    pub fn new(allow_private: bool) -> Result<Self> {
        let inner = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .dns_resolver(Arc::new(GuardedDns { allow_private }))
            .connect_timeout(TIMEOUT)
            .timeout(TIMEOUT)
            .build()
            .map_err(|_| Error::Peer("cannot configure peer HTTP client"))?;
        Ok(Self { inner })
    }

    pub async fn get<T: DeserializeOwned>(&self, base: &Url, path: &str) -> Result<T> {
        // Only internal, fixed relative paths are passed here.
        let url = base
            .join(path)
            .map_err(|_| Error::Invariant("invalid node route"))?;
        let mut response = self
            .inner
            .get(url)
            .send()
            .await
            .map_err(|_| Error::Peer("peer request unavailable"))?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::Http(format!(
                "node returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let length = response.content_length().unwrap_or(0);
        if length > MAX_BODY as u64 {
            return Err(Error::Peer("oversize peer response"));
        }
        let mut bytes = Vec::with_capacity(length as usize);
        while let Some(part) = response
            .chunk()
            .await
            .map_err(|_| Error::Peer("incomplete peer response"))?
        {
            if part.len() > MAX_BODY - bytes.len() {
                return Err(Error::Peer("oversize peer response"));
            }
            bytes.extend_from_slice(&part);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
}

pub(crate) async fn get_json(url: &str, path: &str, allow_private: bool) -> Result<Value> {
    Client::new(allow_private)?
        .get(&base_url(url, allow_private)?, path)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_only_checks_normalized_address_and_origin() {
        for url in [
            "http://example.org",
            "https://127.1",
            "https://2130706433",
            "https://[::ffff:7f00:1]",
            "https://[fc00::1]",
            "https://169.254.169.254",
            "https://100.100.100.200",
            "https://[fe80::1]",
            "https://test.local",
            "https://user@example.org/",
            "https://example.org/path",
            "https://example.org/?a=b",
            "https://example.org/#",
            "https://example.org:0",
        ] {
            assert!(base_url(url, false).is_err(), "{url}");
        }
        assert!(base_url("http://node-b:8080", true).is_ok());
        assert!(base_url("https://example.org", false).is_ok());
        assert!(base_url("https://[2606:4700::1]", false).is_ok());
    }

    #[tokio::test]
    async fn rejects_private_dns_before_connect() {
        let resolver = GuardedDns {
            allow_private: false,
        };
        assert!(resolver
            .resolve("localhost".parse().unwrap())
            .await
            .is_err());
        let resolver = GuardedDns {
            allow_private: true,
        };
        assert!(resolver
            .resolve("localhost".parse().unwrap())
            .await
            .unwrap()
            .any(|addr| addr.ip().is_loopback()));
    }
}
