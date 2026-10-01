//! Transactional application state. Callers must establish consensus finality before committing.
use crate::{
	application::{
		execution::{Account, ExecutionError, Ledger, TransferEffect, TransferOutcome},
		Cursor,
	},
	crypto::Id,
	genesis::Genesis,
};
use redb::{
	Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
};
use std::{
	fs::{File, OpenOptions},
	path::Path,
};

const DATA: TableDefinition<&[u8], &[u8]> = TableDefinition::new("application_v1");
const VERSION: &[u8] = b"\0\0\0\x01";
type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("storage: {0}")]
	Database(#[from] redb::Error),
	#[error("storage I/O: {0}")]
	Io(#[from] std::io::Error),
	#[error("execution: {0}")]
	Execution(#[from] ExecutionError),
	#[error("incompatible application data version")]
	Version,
	#[error("database does not match the supplied genesis")]
	Genesis,
	#[error("inconsistent application data")]
	Corrupt,
	#[error("conflicting decision at an already committed height")]
	Conflict,
	#[error("storage outcome uncertain; close and recover before continuing")]
	RecoveryRequired,
}
fn db<E: Into<redb::Error>>(error: E) -> Error {
	Error::Database(error.into())
}
fn key(prefix: u8, suffix: &[u8]) -> Vec<u8> {
	[vec![prefix], suffix.to_vec()].concat()
}
fn account_bytes(account: Account) -> Vec<u8> {
	[account.balance.to_be_bytes(), account.next_nonce.to_be_bytes()].concat()
}
fn head_bytes(ledger: &Ledger) -> Vec<u8> {
	[ledger.height().to_be_bytes().as_slice(), &ledger.block_id(), &ledger.state_root()].concat()
}

/// Durable inclusion record, not a network-verifiable finality certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
	pub block_id: Id,
	pub outcome: TransferOutcome,
}
impl Receipt {
	fn encode(&self) -> Vec<u8> {
		let o = &self.outcome;
		[
			o.height.to_be_bytes().as_slice(),
			&o.index.to_be_bytes(),
			&self.block_id,
			&o.effect.sender,
			&o.effect.recipient,
			&account_bytes(o.effect.sender_after),
			&account_bytes(o.effect.recipient_after),
		]
		.concat()
	}
	fn decode(tx_id: Id, raw: &[u8]) -> Result<Self> {
		let mut c = Cursor::new(raw);
		let mut parse = || -> crate::crypto::Result<Self> {
			let height = c.u64()?;
			let index = c.u32()?;
			let block_id = c.read()?;
			let effect = TransferEffect {
				tx_id,
				sender: c.read()?,
				recipient: c.read()?,
				sender_after: Account { balance: c.u64()?, next_nonce: c.u64()? },
				recipient_after: Account { balance: c.u64()?, next_nonce: c.u64()? },
			};
			Ok(Self { block_id, outcome: TransferOutcome { height, index, effect } })
		};
		let receipt = parse().map_err(|_| Error::Corrupt)?;
		if !c.is_empty() {
			return Err(Error::Corrupt);
		}
		Ok(receipt)
	}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commit {
	Applied,
	AlreadyApplied,
}

/// Single-process application store. Holding it retains redb's exclusive database lock.
/// Recovery validates execution consistency, not certificates or signing safety.
pub struct Store {
	database: Database,
	ledger: Ledger,
	healthy: bool,
}
impl Store {
	/// Create a new database exclusively. Never replaces or reinitializes an existing file.
	/// Use a private, trusted node directory; interrupted initialization fails closed on open.
	pub fn create(path: &Path, genesis: &Genesis) -> Result<Self> {
		let ledger = Ledger::from_genesis(genesis)?;
		let mut options = OpenOptions::new();
		options.read(true).write(true).create_new(true);
		#[cfg(unix)]
		{
			use std::os::unix::fs::OpenOptionsExt;
			options.mode(0o600);
		}
		let database = Database::builder()
			.set_cache_size(8 * 1024 * 1024)
			.create_file(options.open(path)?)
			.map_err(db)?;
		let mut tx = database.begin_write().map_err(db)?;
		tx.set_durability(Durability::Immediate).map_err(db)?;
		tx.set_two_phase_commit(true);
		{
			let mut table = tx.open_table(DATA).map_err(db)?;
			table.insert(b"version".as_slice(), VERSION).map_err(db)?;
			table
				.insert(
					b"genesis".as_slice(),
					genesis.encode().map_err(|_| Error::Genesis)?.as_slice(),
				)
				.map_err(db)?;
			table.insert(b"head".as_slice(), head_bytes(&ledger).as_slice()).map_err(db)?;
			for (id, account) in ledger.accounts() {
				table
					.insert(key(b'a', id).as_slice(), account_bytes(*account).as_slice())
					.map_err(db)?;
			}
		}
		tx.commit().map_err(db)?;
		// Durably publish the new filename as well as its contents on Unix.
		#[cfg(unix)]
		File::open(path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")))?
			.sync_all()?;
		Ok(Self { database, ledger, healthy: true })
	}
	/// Open existing data and replay every retained block, checking all derived rows.
	pub fn open(path: &Path, genesis: &Genesis) -> Result<Self> {
		let mut ledger = Ledger::from_genesis(genesis)?;
		let database =
			Database::builder().set_cache_size(8 * 1024 * 1024).open(path).map_err(db)?;
		{
			let read = database.begin_read().map_err(db)?;
			let table = read.open_table(DATA).map_err(db)?;
			let get = |key: &[u8]| -> Result<Vec<u8>> {
				Ok(table.get(key).map_err(db)?.ok_or(Error::Corrupt)?.value().to_vec())
			};
			if get(b"version")? != VERSION {
				return Err(Error::Version);
			}
			if get(b"genesis")? != genesis.encode().map_err(|_| Error::Genesis)? {
				return Err(Error::Genesis);
			}
			let head = get(b"head")?;
			if head.len() != 72 {
				return Err(Error::Corrupt);
			}
			let height = u64::from_be_bytes(head[..8].try_into().map_err(|_| Error::Corrupt)?);
			// A corrupt height cannot trigger unbounded missing-block lookups.
			if height > table.len().map_err(db)? {
				return Err(Error::Corrupt);
			}
			let mut rows = 3u64;
			for h in 1..=height {
				let raw = get(&key(b'b', &h.to_be_bytes()))?;
				let validated = ledger.validate_block(&raw)?;
				for outcome in validated.outcomes() {
					let receipt = Receipt {
						block_id: validated.block().header.id(),
						outcome: outcome.clone(),
					};
					if get(&key(b'r', &outcome.effect.tx_id))? != receipt.encode() {
						return Err(Error::Corrupt);
					}
					rows = rows.checked_add(1).ok_or(Error::Corrupt)?;
				}
				rows = rows.checked_add(1).ok_or(Error::Corrupt)?;
				ledger = validated.candidate_state().clone();
			}
			if head != head_bytes(&ledger) {
				return Err(Error::Corrupt);
			}
			for (id, account) in ledger.accounts() {
				if get(&key(b'a', id))? != account_bytes(*account) {
					return Err(Error::Corrupt);
				}
				rows = rows.checked_add(1).ok_or(Error::Corrupt)?;
			}
			if rows != table.len().map_err(db)? {
				return Err(Error::Corrupt);
			}
		}
		Ok(Self { database, ledger, healthy: true })
	}
	pub fn ledger(&self) -> Result<&Ledger> {
		self.check()?;
		Ok(&self.ledger)
	}
	fn check(&self) -> Result<()> {
		if self.healthy {
			Ok(())
		} else {
			Err(Error::RecoveryRequired)
		}
	}
	fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
		self.check()?;
		let read = self.database.begin_read().map_err(db)?;
		let table = read.open_table(DATA).map_err(db)?;
		Ok(table.get(key).map_err(db)?.map(|value| value.value().to_vec()))
	}
	pub fn block(&self, height: u64) -> Result<Option<Vec<u8>>> {
		self.get(&key(b'b', &height.to_be_bytes()))
	}
	pub fn receipt(&self, tx_id: &Id) -> Result<Option<Receipt>> {
		self.get(&key(b'r', tx_id))?.map(|raw| Receipt::decode(*tx_id, &raw)).transpose()
	}
	/// Commit only after the consensus adapter authenticates a decision for these exact bytes.
	/// This method checks execution, NOT a quorum certificate. Never expose it directly to RPC.
	/// Acknowledge Malachite only after success. On error, stop and reconcile before signing.
	pub fn commit_decided(&mut self, height: u64, bytes: &[u8]) -> Result<Commit> {
		self.commit_with_hook(height, bytes, |_| {})
	}
	fn commit_with_hook(
		&mut self,
		height: u64,
		bytes: &[u8],
		mut hook: impl FnMut(&str),
	) -> Result<Commit> {
		self.check()?;
		if height == 0 {
			return Err(Error::Conflict);
		}
		if height <= self.ledger.height() {
			return if self.block(height)?.as_deref() == Some(bytes) {
				Ok(Commit::AlreadyApplied)
			} else {
				Err(Error::Conflict)
			};
		}
		let validated = self.ledger.validate_block(bytes)?;
		if validated.block().header.height != height {
			return Err(ExecutionError::WrongHeight.into());
		}
		// Any write/commit error leaves the outcome uncertain until disk recovery.
		self.healthy = false;
		let mut tx = self.database.begin_write().map_err(db)?;
		tx.set_durability(Durability::Immediate).map_err(db)?;
		tx.set_two_phase_commit(true);
		{
			let mut table = tx.open_table(DATA).map_err(db)?;
			if table.get(b"head".as_slice()).map_err(db)?.ok_or(Error::Corrupt)?.value()
				!= head_bytes(&self.ledger)
			{
				return Err(Error::Corrupt);
			}
			table.insert(key(b'b', &height.to_be_bytes()).as_slice(), bytes).map_err(db)?;
			hook("block");
			for outcome in validated.outcomes() {
				let effect = &outcome.effect;
				table
					.insert(
						key(b'a', &effect.sender).as_slice(),
						account_bytes(effect.sender_after).as_slice(),
					)
					.map_err(db)?;
				hook("debit");
				table
					.insert(
						key(b'a', &effect.recipient).as_slice(),
						account_bytes(effect.recipient_after).as_slice(),
					)
					.map_err(db)?;
				hook("credit");
				let receipt =
					Receipt { block_id: validated.block().header.id(), outcome: outcome.clone() };
				if table
					.insert(key(b'r', &effect.tx_id).as_slice(), receipt.encode().as_slice())
					.map_err(db)?
					.is_some()
				{
					return Err(Error::Corrupt);
				}
				hook("receipt");
			}
			table
				.insert(b"head".as_slice(), head_bytes(validated.candidate_state()).as_slice())
				.map_err(db)?;
			hook("head");
		}
		hook("before_commit");
		tx.commit().map_err(db)?;
		hook("after_commit");
		self.ledger = validated.candidate_state().clone();
		self.healthy = true;
		Ok(Commit::Applied)
	}
}
#[cfg(test)]
mod tests;
