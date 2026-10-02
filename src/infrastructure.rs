//! Bounded configuration loading and protected local identity files (Unix M1 profile).
use crate::{
	crypto::{Error, Id, KeyRole, Result, SecretKey},
	genesis::{parse_id, Genesis, MAX_GENESIS_JSON},
};
use serde::Deserialize;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
	collections::BTreeSet,
	fs::{self, File, OpenOptions},
	io::{Read, Write},
	net::SocketAddr,
	path::{Path, PathBuf},
};
use zeroize::Zeroizing;

pub fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>> {
	let mut bytes = Vec::new();
	File::open(path)?.take((max + 1) as u64).read_to_end(&mut bytes)?;
	if bytes.len() > max {
		return Err(Error::Encoding);
	}
	Ok(bytes)
}
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
	let mut options = OpenOptions::new();
	options.write(true).create_new(true);
	#[cfg(unix)]
	options.mode(0o600);
	let mut file = options.open(path)?;
	file.write_all(bytes)?;
	file.sync_all()?;
	File::open(path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")))?
		.sync_all()?;
	Ok(())
}
#[cfg(unix)]
pub fn save_key(path: &Path, key: &SecretKey) -> Result<()> {
	let mut bytes = Zeroizing::new(Vec::with_capacity(43));
	bytes.extend(b"RINKEY01");
	bytes.extend(1u16.to_be_bytes());
	bytes.push(key.role() as u8);
	bytes.extend_from_slice(key.seed().as_ref());
	write_new(path, &bytes)
}
#[cfg(not(unix))]
pub fn save_key(_: &Path, _: &SecretKey) -> Result<()> {
	Err(Error::Config("protected key files currently require Unix"))
}

#[cfg(unix)]
pub fn load_key(path: &Path, role: KeyRole) -> Result<SecretKey> {
	let mut file = OpenOptions::new()
		.read(true)
		.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
		.open(path)?;
	let meta = file.metadata()?;
	if !meta.is_file() || meta.permissions().mode() & 0o777 != 0o600 || meta.nlink() != 1 {
		return Err(Error::Config(
			"key file must be a private regular file with mode 0600 and one link",
		));
	}
	let mut bytes = Zeroizing::new([0u8; 43]);
	file.read_exact(bytes.as_mut())?;
	let mut extra = [0];
	if file.read(&mut extra)? != 0 {
		return Err(Error::Encoding);
	}
	if &bytes[..8] != b"RINKEY01" || bytes[8..10] != 1u16.to_be_bytes() {
		return Err(Error::Encoding);
	}
	let stored_role = KeyRole::try_from(bytes[10])?;
	if stored_role != role {
		return Err(Error::Role);
	}
	let mut seed = Zeroizing::new([0; 32]);
	seed.copy_from_slice(&bytes[11..]);
	Ok(SecretKey::from_seed(&seed, role))
}
#[cfg(not(unix))]
pub fn load_key(_: &Path, _: KeyRole) -> Result<SecretKey> {
	Err(Error::Config("protected key files currently require Unix"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
	version: u16,
	genesis: PathBuf,
	expected_chain_id: String,
	validator_key: PathBuf,
	network_key: PathBuf,
	data_dir: PathBuf,
	listen: SocketAddr,
	rpc_listen: Option<SocketAddr>,
	peers: Vec<Peer>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Peer {
	address: SocketAddr,
	public_key: String,
}
// Secret keys deliberately do not implement Debug.
pub struct NodeConfig {
	pub genesis: Genesis,
	pub validator: SecretKey,
	pub network: SecretKey,
	pub data_dir: PathBuf,
	pub listen: SocketAddr,
	pub rpc_listen: Option<SocketAddr>,
	pub peers: Vec<(SocketAddr, Id)>,
}
impl NodeConfig {
	pub fn load(path: &Path) -> Result<Self> {
		let bytes = read_bounded(path, 64 * 1024)?;
		let text = std::str::from_utf8(&bytes).map_err(|_| Error::Encoding)?;
		let f: ConfigFile = toml::from_str(text).map_err(|_| Error::Config("invalid node TOML"))?;
		if f.version != 1 {
			return Err(Error::Version);
		}
		let parent =
			fs::canonicalize(path)?.parent().ok_or(Error::Config("config parent"))?.to_path_buf();
		let genesis =
			Genesis::from_json(&read_bounded(&parent.join(f.genesis), MAX_GENESIS_JSON)?)?;
		if genesis.chain_id()? != parse_id(&f.expected_chain_id)? {
			return Err(Error::Chain);
		}
		let validator = load_key(&parent.join(f.validator_key), KeyRole::Validator)?;
		let network = load_key(&parent.join(f.network_key), KeyRole::Network)?;
		if !genesis.validators.contains(&validator.public_key()) {
			return Err(Error::Config("validator is not in genesis"));
		}
		if validator.public_key() == network.public_key() {
			return Err(Error::Config("validator and network identities must differ"));
		}
		if f.listen.port() == 0 || f.listen.ip().is_multicast() || f.peers.len() > 64 {
			return Err(Error::Config("invalid network configuration"));
		}
		if f.rpc_listen.is_some_and(|a| !a.ip().is_loopback() || a.port() == 0 || a == f.listen) {
			return Err(Error::Config("RPC requires a separate nonzero loopback address"));
		}
		let mut peers = Vec::new();
		let mut addresses = BTreeSet::new();
		let mut keys = BTreeSet::new();
		for peer in f.peers {
			let key = parse_id(&peer.public_key)?;
			crate::crypto::public_key(&key)?;
			if peer.address.port() == 0
				|| peer.address.ip().is_unspecified()
				|| peer.address.ip().is_multicast()
				|| peer.address == f.listen
				|| key == network.public_key()
				|| !addresses.insert(peer.address)
				|| !keys.insert(key)
			{
				return Err(Error::Config("duplicate, self or invalid peer"));
			}
			peers.push((peer.address, key));
		}
		let config = Self {
			genesis,
			validator,
			network,
			data_dir: parent.join(f.data_dir),
			listen: f.listen,
			rpc_listen: f.rpc_listen,
			peers,
		};
		config.check_directory()?;
		Ok(config)
	}
	fn binding(&self) -> Result<Vec<u8>> {
		Ok([
			b"RINPQC/DATADIR/v1\0".as_slice(),
			&self.genesis.chain_id()?,
			&self.validator.public_key(),
			&self.network.public_key(),
		]
		.concat())
	}
	pub fn check_directory(&self) -> Result<()> {
		match fs::symlink_metadata(&self.data_dir) {
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
			Err(e) => return Err(e.into()),
			Ok(meta) => {
				if !meta.is_dir() || meta.file_type().is_symlink() {
					return Err(Error::Config("data directory must be a real directory"));
				}
				#[cfg(unix)]
				if meta.permissions().mode() & 0o777 != 0o700 {
					return Err(Error::Config("data directory must have mode 0700"));
				}
			},
		}
		let marker = self.data_dir.join("identity.bin");
		if read_bounded(&marker, 128)? != self.binding()? {
			return Err(Error::Config("data directory belongs to another identity or chain"));
		}
		Ok(())
	}
	#[cfg(unix)]
	pub fn initialize(&self) -> Result<()> {
		match fs::DirBuilder::new().mode(0o700).create(&self.data_dir) {
			Ok(()) => write_new(&self.data_dir.join("identity.bin"), &self.binding()?),
			Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => self.check_directory(),
			Err(e) => Err(e.into()),
		}
	}
	#[cfg(not(unix))]
	pub fn initialize(&self) -> Result<()> {
		Err(Error::Config("data directory initialization currently requires Unix"))
	}
}
