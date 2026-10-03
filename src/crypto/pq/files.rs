//! Protected plaintext key files. Parent directories must be trusted and owner-controlled.
use super::{Error, KeyRole, Result, SecretKey, SECRET_FILE_BYTES};
use std::path::Path;

#[cfg(unix)]
pub fn save_new(path: &Path, key: &SecretKey) -> Result<()> {
	crate::infrastructure::write_new(path, &key.export())
}
#[cfg(unix)]
pub fn load(path: &Path, role: KeyRole) -> Result<SecretKey> {
	use std::{
		fs::OpenOptions,
		io::Read,
		os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
	};
	use zeroize::Zeroizing;
	let mut file = OpenOptions::new()
		.read(true)
		.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
		.open(path)?;
	let meta = file.metadata()?;
	if !meta.is_file() || meta.permissions().mode() & 0o7777 != 0o600 || meta.nlink() != 1 {
		return Err(Error::Config(
			"key file must be a private regular file with mode 0600 and one link",
		));
	}
	if meta.len() != SECRET_FILE_BYTES as u64 {
		return Err(Error::Encoding);
	}
	// Fixed allocation: hostile input cannot grow a secret-bearing Vec and leave unwiped copies.
	let mut bytes = Zeroizing::new(vec![0; SECRET_FILE_BYTES + 1]);
	file.read_exact(&mut bytes[..SECRET_FILE_BYTES])?;
	if file.read(&mut bytes[SECRET_FILE_BYTES..])? != 0 {
		return Err(Error::Encoding);
	}
	SecretKey::import(&bytes[..SECRET_FILE_BYTES], role)
}
#[cfg(not(unix))]
pub fn save_new(_: &Path, _: &SecretKey) -> Result<()> {
	Err(Error::Config("protected key files currently require Unix"))
}
#[cfg(not(unix))]
pub fn load(_: &Path, _: KeyRole) -> Result<SecretKey> {
	Err(Error::Config("protected key files currently require Unix"))
}
