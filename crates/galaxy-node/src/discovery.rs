use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read, Write},
    path::Path,
    sync::Arc,
    time::Duration,
};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use futures_util::{stream, StreamExt};
use galaxy_protocol::{canonical_json, Hash32};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use url::Url;
use zeroize::Zeroizing;

use crate::{
    client::{self, Client},
    config::MAX_PEERS,
    db::{Db, Range},
    unix_now, Config, Error, Result, API_VERSION, PROTOCOL_VERSION, SOFTWARE_VERSION,
};

const DESCRIPTOR_ROUTE: &str = ".well-known/galaxy-node.json";
const KEY_FILE: &str = "node.ed25519";
const DOMAIN: &[u8] = b"GALAXY/NODE/v1\0";
const ANNOUNCED_TTL: u64 = 15 * 60;
pub(crate) const MAX_TTL: u64 = 24 * 60 * 60;
const MAX_DEPTH: usize = 2;
const MAX_ENDPOINTS: usize = 4;
const DESCRIPTOR_RANGES: usize = 32;

pub(crate) fn peer_url(text: &str, allow_private: bool) -> Result<Url> {
    client::base_url(text, allow_private)
}

pub(crate) struct Identity(SigningKey);

impl Identity {
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let path = dir.join(KEY_FILE);
        match read_key(&path) {
            Ok(key) => return Ok(key),
            Err(Error::Io(e)) if e.kind() == ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        let mut secret = Zeroizing::new([0u8; 32]);
        let mut suffix = [0u8; 16];
        getrandom::fill(&mut *secret)
            .and_then(|()| getrandom::fill(&mut suffix))
            .map_err(|_| Error::Invariant("system random generator unavailable"))?;
        let temporary = dir.join(format!(".node-key-{}", hex::encode(suffix)));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        let result = (|| {
            file.write_all(&*secret)?;
            file.sync_all()?;
            match fs::hard_link(&temporary, &path) {
                Ok(()) => {
                    #[cfg(unix)]
                    File::open(dir)?.sync_all()?;
                    Ok(Self(SigningKey::from_bytes(&secret)))
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => read_key(&path),
                Err(e) => Err(e.into()),
            }
        })();
        drop(file);
        fs::remove_file(temporary)?;
        result
    }

    fn id(&self) -> Hash32 {
        self.0.verifying_key().to_bytes()
    }

    pub fn descriptor(&self, config: &Config, db: &Db) -> Result<Descriptor> {
        let mut indexed_ranges = db.ranges("", 0, DESCRIPTOR_RANGES + 1)?;
        let ranges_truncated = indexed_ranges.len() > DESCRIPTOR_RANGES;
        indexed_ranges.truncate(DESCRIPTOR_RANGES);
        let (indexed_from, indexed_through) = if ranges_truncated {
            db.scope_bounds("")?
        } else {
            (
                indexed_ranges.first().map(|r| r.from),
                indexed_ranges.last().map(|r| r.through),
            )
        };
        let mut descriptor = Descriptor {
            protocol: API_VERSION.into(),
            protocol_version: PROTOCOL_VERSION.into(),
            node_id: hex::encode(self.id()),
            network: config.network.name.clone(),
            network_id: config.network.network_id.as_u32(),
            software_version: SOFTWARE_VERSION.into(),
            endpoints: vec![config.public_url.to_string()],
            capabilities: vec![
                "photo-lookup".into(),
                "proof-lookup".into(),
                "ledger-index".into(),
                "peer-exchange".into(),
            ],
            indexed_from,
            indexed_through,
            indexed_ranges,
            ranges_truncated,
            expires_at: unix_now() + ANNOUNCED_TTL,
            signature: String::new(),
        };
        descriptor.signature = hex::encode(self.0.sign(&descriptor.message()?).to_bytes());
        Ok(descriptor)
    }
}

fn read_key(path: &Path) -> Result<Identity> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(Error::Config("node identity must be a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::Config(
                "node identity permissions must restrict access to the owner",
            ));
        }
    }
    let mut file = File::open(path)?;
    let mut secret = Zeroizing::new([0u8; 32]);
    file.read_exact(&mut *secret)?;
    if file.read(&mut [0u8; 1])? != 0 {
        return Err(Error::Config("invalid node identity length"));
    }
    Ok(Identity(SigningKey::from_bytes(&secret)))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Descriptor {
    protocol: String,
    protocol_version: String,
    node_id: String,
    network: String,
    network_id: u32,
    software_version: String,
    endpoints: Vec<String>,
    capabilities: Vec<String>,
    indexed_from: Option<u32>,
    indexed_through: Option<u32>,
    indexed_ranges: Vec<Range>,
    ranges_truncated: bool,
    expires_at: u64,
    signature: String,
}

impl Descriptor {
    pub fn node_id(&self) -> &str {
        &self.node_id
    }
    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }

    fn message(&self) -> Result<Vec<u8>> {
        let mut json = serde_json::to_value(self)?;
        json.as_object_mut()
            .ok_or(Error::Invariant("descriptor is not an object"))?
            .remove("signature");
        let canonical = canonical_json::to_vec(&json)
            .map_err(|_| Error::Peer("invalid canonical descriptor"))?;
        let mut message = Vec::with_capacity(DOMAIN.len() + canonical.len());
        message.extend_from_slice(DOMAIN);
        message.extend_from_slice(&canonical);
        Ok(message)
    }

    pub(crate) fn verify(&self, config: &Config, now: u64) -> Result<(Hash32, Vec<Url>)> {
        if self.protocol != API_VERSION
            || self.protocol_version != PROTOCOL_VERSION
            || self.network != config.network.name
            || self.network_id != config.network.network_id.as_u32()
            || self.expires_at <= now
            || self.expires_at > now + MAX_TTL
            || self.endpoints.is_empty()
            || self.endpoints.len() > MAX_ENDPOINTS
            || self.capabilities.len() > 16
            || self
                .capabilities
                .iter()
                .any(|s| s.is_empty() || s.len() > 32)
            || self.software_version.is_empty()
            || self.software_version.len() > 64
            || self.indexed_ranges.len() > DESCRIPTOR_RANGES
            || self.indexed_ranges.first().map(|r| r.from) != self.indexed_from
            || self
                .indexed_ranges
                .iter()
                .any(|r| r.from == 0 || r.through < r.from)
            || self
                .indexed_ranges
                .windows(2)
                .any(|pair| pair[0].through.saturating_add(1) >= pair[1].from)
            || (if self.ranges_truncated {
                self.indexed_ranges.len() != DESCRIPTOR_RANGES
                    || self.indexed_through <= self.indexed_ranges.last().map(|r| r.through)
            } else {
                self.indexed_through != self.indexed_ranges.last().map(|r| r.through)
            })
        {
            return Err(Error::Peer(
                "incompatible, expired, or out-of-bounds descriptor",
            ));
        }
        let id = lower_hex(&self.node_id)?;
        let signature = lower_hex(&self.signature)?;
        let key = VerifyingKey::from_bytes(&id).map_err(|_| Error::Peer("invalid peer key"))?;
        key.verify_strict(&self.message()?, &Signature::from_bytes(&signature))
            .map_err(|_| Error::Peer("invalid descriptor signature"))?;
        let urls = self
            .endpoints
            .iter()
            .map(|text| peer_url(text, config.node.allow_private_peers))
            .collect::<Result<Vec<_>>>()?;
        Ok((id, urls))
    }
}

fn lower_hex<const N: usize>(text: &str) -> Result<[u8; N]> {
    let mut out = [0u8; N];
    hex::decode_to_slice(text, &mut out).map_err(|_| Error::Peer("invalid descriptor encoding"))?;
    if text.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(Error::Peer("invalid descriptor encoding"));
    }
    Ok(out)
}

struct Seed {
    url: Url,
    id: Option<Hash32>,
}

fn push(queue: &mut Vec<Seed>, visited: &mut HashSet<Url>, seed: Seed, config: &Config) {
    if visited.len() < MAX_PEERS
        && seed.url != config.public_url
        && visited.insert(seed.url.clone())
    {
        queue.push(seed);
    }
}

#[derive(Deserialize)]
struct PeerExchange {
    peers: Vec<Descriptor>,
}

async fn probe(
    client: &Client,
    seed: Seed,
    config: &Config,
    self_id: Hash32,
    exchange: bool,
) -> Option<(Hash32, Descriptor, Vec<Descriptor>)> {
    let descriptor: Descriptor = client.get(&seed.url, DESCRIPTOR_ROUTE).await.ok()?;
    let (id, _) = descriptor.verify(config, unix_now()).ok()?;
    if self_id == id || seed.id.is_some_and(|expected| id != expected) {
        return None;
    }
    let mut peers = Vec::new();
    if exchange {
        if let Ok(result) = client.get::<PeerExchange>(&seed.url, "v1/peers").await {
            if result.peers.len() <= MAX_PEERS {
                peers = result.peers;
            }
        }
    }
    Some((id, descriptor, peers))
}

async fn discover(config: &Config, db: &Db, identity: &Identity, client: &Client) -> Result<()> {
    let now = unix_now();
    let own_id = identity.id();
    let mut seeds = Vec::new();
    let mut visited = HashSet::new();
    let mut found = HashMap::new();
    let mut cached = Vec::new();
    for text in config
        .node
        .manual_peers
        .iter()
        .chain(&config.node.bootstrap_peers)
    {
        push(
            &mut seeds,
            &mut visited,
            Seed {
                url: peer_url(text, config.node.allow_private_peers)?,
                id: None,
            },
            config,
        );
    }
    for json in db.peer_documents(now as i64)? {
        if let Ok(descriptor) = serde_json::from_str::<Descriptor>(&json) {
            if let Ok((id, urls)) = descriptor.verify(config, now) {
                if id == own_id {
                    continue;
                }
                for url in urls {
                    push(&mut seeds, &mut visited, Seed { url, id: Some(id) }, config);
                }
                cached.push((id, descriptor));
            }
        }
    }
    if config.node.xrpl_announcements {
        for (text, id) in db.announcements(now as i64)? {
            if id != own_id {
                if let Ok(url) = peer_url(&text, config.node.allow_private_peers) {
                    push(&mut seeds, &mut visited, Seed { url, id: Some(id) }, config);
                }
            }
        }
    }
    for depth in 0..=MAX_DEPTH {
        if seeds.is_empty() {
            break;
        }
        let mut wave = stream::iter(std::mem::take(&mut seeds))
            .map(|seed| probe(client, seed, config, own_id, depth < MAX_DEPTH))
            .buffered(8);
        while let Some(result) = wave.next().await {
            let Some((id, descriptor, peers)) = result else {
                continue;
            };
            if found.len() < MAX_PEERS || found.contains_key(&id) {
                found.insert(id, descriptor);
            }
            for peer in peers {
                if visited.len() == MAX_PEERS {
                    break;
                }
                if let Ok((peer_id, urls)) = peer.verify(config, unix_now()) {
                    if peer_id != own_id && !found.contains_key(&peer_id) {
                        for url in urls {
                            push(
                                &mut seeds,
                                &mut visited,
                                Seed {
                                    url,
                                    id: Some(peer_id),
                                },
                                config,
                            );
                        }
                    }
                }
            }
        }
    }
    for (id, descriptor) in cached {
        if found.len() == MAX_PEERS {
            break;
        }
        found.entry(id).or_insert(descriptor);
    }
    let now = unix_now();
    let peers: Vec<_> = found
        .into_values()
        .filter(|peer| peer.expires_at() > now)
        .collect();
    db.replace_peers(&peers)
}

pub(crate) async fn run(config: Arc<Config>, db: Arc<Db>, identity: Arc<Identity>) -> Result<()> {
    let client = Client::new(config.node.allow_private_peers)?;
    loop {
        discover(&config, &db, &identity, &client).await?;
        sleep(Duration::from_secs(
            config.node.limits.discovery_interval_secs,
        ))
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_same_private_key_without_replacing_it() {
        let tmp = tempfile::tempdir().unwrap();
        let key = Identity::load_or_create(tmp.path()).unwrap();
        assert_eq!(key.id(), Identity::load_or_create(tmp.path()).unwrap().id());
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }
}
