//! Fresh M2 bootstrap directories. Runtime database/WAL adapters are integrated separately.
//! Parent directories must be trusted and owner-controlled, as for PQ secret files.
use super::{
	genesis::{Genesis, MAX_GENESIS_BYTES},
	*,
};
use std::path::Path;

fn binding(
	genesis: &Genesis,
	validator: &pq::PublicKey,
	network: &pq::PublicKey,
) -> Result<Vec<u8>> {
	genesis.validate()?;
	if validator.role() != KeyRole::Validator
		|| network.role() != KeyRole::Network
		|| !genesis.validators.contains(validator)
		|| genesis.validators.iter().any(|key| key.to_bytes() == network.to_bytes())
	{
		return Err(Error::Role);
	}
	let mut out = prefix(7);
	out.extend(genesis.chain_id()?.0);
	out.extend(validator.to_bytes());
	out.extend(network.to_bytes());
	Ok(out)
}
#[cfg(unix)]
pub fn initialize(
	path: &Path,
	genesis: &Genesis,
	validator: &pq::PublicKey,
	network: &pq::PublicKey,
) -> Result<()> {
	use std::{
		fs,
		os::unix::fs::{DirBuilderExt, PermissionsExt},
	};
	// Validate everything before any filesystem mutation. Never adopt even an empty existing path.
	let marker = binding(genesis, validator, network)?;
	let genesis_bytes = genesis.encode()?;
	fs::DirBuilder::new().mode(0o700).create(path)?;
	if fs::metadata(path)?.permissions().mode() & 0o7777 != 0o700 {
		return Err(Error::Config("M2 directory must have mode 0700"));
	}
	crate::infrastructure::write_new(&path.join("genesis.bin"), &genesis_bytes)?;
	// The marker is last: an interrupted initialization cannot be mistaken for complete state.
	crate::infrastructure::write_new(&path.join("profile.bin"), &marker)?;
	fs::File::open(path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")))?
		.sync_all()?;
	Ok(())
}
#[cfg(unix)]
pub fn check(
	path: &Path,
	genesis: &Genesis,
	validator: &pq::PublicKey,
	network: &pq::PublicKey,
) -> Result<()> {
	use std::{
		fs,
		io::Read,
		os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
	};
	let marker = binding(genesis, validator, network)?;
	let metadata = fs::symlink_metadata(path)?;
	if !metadata.is_dir()
		|| metadata.file_type().is_symlink()
		|| metadata.permissions().mode() & 0o7777 != 0o700
	{
		return Err(Error::Config("M2 directory must be a real directory with mode 0700"));
	}
	// This bootstrap profile has no database/WAL yet. Do not accept mixed M1/M2 directories.
	for entry in fs::read_dir(path)? {
		let name = entry?.file_name();
		if name != "genesis.bin" && name != "profile.bin" {
			return Err(Error::Config("unexpected file in M2 bootstrap directory"));
		}
	}
	fn read(path: &Path, max: usize) -> Result<Vec<u8>> {
		let mut file = fs::OpenOptions::new()
			.read(true)
			.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
			.open(path)?;
		let meta = file.metadata()?;
		if !meta.is_file()
			|| meta.nlink() != 1
			|| meta.permissions().mode() & 0o7777 != 0o600
			|| meta.len() > max as u64
		{
			return Err(Error::Encoding);
		}
		let mut bytes = Vec::new();
		(&mut file).take((max + 1) as u64).read_to_end(&mut bytes)?;
		if bytes.len() > max {
			return Err(Error::Encoding);
		}
		Ok(bytes)
	}
	if read(&path.join("profile.bin"), marker.len())? != marker {
		return Err(Error::Config("M2 directory identity/profile mismatch"));
	}
	let bytes = read(&path.join("genesis.bin"), MAX_GENESIS_BYTES)?;
	let decoded = Genesis::decode_for_chain(&bytes, genesis.chain_id()?)?;
	if &decoded != genesis {
		return Err(Error::Chain);
	}
	Ok(())
}
#[cfg(not(unix))]
pub fn initialize(_: &Path, _: &Genesis, _: &pq::PublicKey, _: &pq::PublicKey) -> Result<()> {
	Err(Error::Config("M2 bootstrap directories currently require Unix"))
}
#[cfg(not(unix))]
pub fn check(_: &Path, _: &Genesis, _: &pq::PublicKey, _: &pq::PublicKey) -> Result<()> {
	Err(Error::Config("M2 bootstrap directories currently require Unix"))
}
