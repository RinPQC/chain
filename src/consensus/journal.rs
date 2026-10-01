//! Durable signed-message guard, undecided payloads and finality certificates.
use super::{
	codec::Codec,
	engine::app::{
		consensus::Input,
		engine::wal::log_entries,
		types::{codec::Codec as _, core::CommitCertificate, SignedConsensusMsg},
		wal,
	},
	types::*,
};
use crate::{crypto::Id, infrastructure::write_new};
use eyre::{ensure, eyre, Result};
use redb::{Database, Durability, ReadableDatabase, ReadableTable, TableDefinition};
use std::{
	collections::BTreeSet,
	fs::File,
	path::Path,
	sync::atomic::{AtomicBool, Ordering},
};
const DATA: TableDefinition<&[u8], &[u8]> = TableDefinition::new("consensus_v1");

pub struct Journal {
	database: Database,
	pub codec: Codec,
	healthy: AtomicBool,
}
impl Journal {
	pub fn create(path: &Path, codec: Codec, validator: Id) -> Result<Self> {
		write_new(path, &[])?;
		let database = Database::builder()
			.set_cache_size(8 * 1024 * 1024)
			.create_file(File::options().read(true).write(true).open(path)?)?;
		let journal = Self { database, codec, healthy: AtomicBool::new(true) };
		journal.put_once(
			b"identity",
			&[b"RINPQC-JOURNAL-v1".as_slice(), &journal.codec.chain, &validator].concat(),
		)?;
		Ok(journal)
	}
	pub fn open(path: &Path, codec: Codec, validator: Id) -> Result<Self> {
		let journal = Self {
			database: Database::builder().set_cache_size(8 * 1024 * 1024).open(path)?,
			codec,
			healthy: AtomicBool::new(true),
		};
		ensure!(
			journal.get(b"identity")?.as_deref()
				== Some(
					[b"RINPQC-JOURNAL-v1".as_slice(), &journal.codec.chain, &validator]
						.concat()
						.as_slice()
				),
			"incompatible consensus journal identity/version"
		);
		Ok(journal)
	}
	pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
		ensure!(self.healthy.load(Ordering::SeqCst), "consensus journal requires recovery");
		let tx = self.database.begin_read()?;
		let table = tx.open_table(DATA)?;
		Ok(table.get(key)?.map(|raw| raw.value().to_vec()))
	}
	pub fn put_once(&self, key: &[u8], value: &[u8]) -> Result<()> {
		ensure!(self.healthy.load(Ordering::SeqCst), "consensus journal requires recovery");
		let result = (|| -> Result<()> {
			let mut tx = self.database.begin_write()?;
			tx.set_durability(Durability::Immediate)?;
			tx.set_two_phase_commit(true);
			{
				let mut table = tx.open_table(DATA)?;
				if let Some(old) = table.get(key)? {
					ensure!(old.value() == value, "conflicting immutable journal entry");
					return Ok(());
				}
				table.insert(key, value)?;
			}
			tx.commit()?;
			Ok(())
		})();
		if result.is_err() {
			self.healthy.store(false, Ordering::SeqCst);
		}
		result
	}
	pub fn signed(&self, slot: &[u8], bytes: &[u8]) -> Result<()> {
		ensure!(self.healthy.load(Ordering::SeqCst), "consensus journal requires recovery");
		let result = (|| -> Result<()> {
			let mut tx = self.database.begin_write()?;
			tx.set_durability(Durability::Immediate)?;
			tx.set_two_phase_commit(true);
			{
				let mut table = tx.open_table(DATA)?;
				let key = [b"s".as_slice(), slot].concat();
				if let Some(old) = table.get(key.as_slice())? {
					ensure!(old.value() == bytes, "refusing conflicting signature");
					return Ok(());
				}
				if let Some(last) = table.get(b"watermark".as_slice())? {
					ensure!(slot > last.value(), "refusing regressed signing slot");
				}
				table.insert(key.as_slice(), bytes)?;
				table.insert(b"watermark".as_slice(), slot)?;
			}
			tx.commit()?;
			Ok(())
		})();
		if result.is_err() {
			self.healthy.store(false, Ordering::SeqCst);
		}
		result
	}
	pub fn block(&self, value: Value) -> Result<Option<Vec<u8>>> {
		self.get(&[b"b".as_slice(), &value.0].concat())
	}
	pub fn save_part(&self, part: &Part) -> Result<()> {
		self.put_once(&[b"b".as_slice(), &part.proposal.value.0].concat(), &part.block)?;
		let key = [
			b"p".as_slice(),
			&part.proposal.height.0.to_be_bytes(),
			&part.proposal.round.as_u32().ok_or_else(|| eyre!("nil round"))?.to_be_bytes(),
			&crate::crypto::hash(b"RINPQC/PART-DESCRIPTOR/v1\0", &borsh::to_vec(&part.proposal)?),
		]
		.concat();
		self.put_once(&key, &self.codec.encode(part)?)
	}
	pub fn parts(
		&self,
		height: Height,
		round: super::engine::app::types::core::Round,
	) -> Result<Vec<Part>> {
		let prefix = [
			b"p".as_slice(),
			&height.0.to_be_bytes(),
			&round.as_u32().ok_or_else(|| eyre!("nil round"))?.to_be_bytes(),
		]
		.concat();
		let tx = self.database.begin_read()?;
		let table = tx.open_table(DATA)?;
		let mut parts = Vec::new();
		for row in table.range(prefix.as_slice()..)? {
			let (key, value) = row?;
			if !key.value().starts_with(&prefix) {
				break;
			}
			ensure!(parts.len() < 4, "too many persisted proposals for a round");
			parts.push(self.codec.decode(value.value().to_vec().into())?);
		}
		Ok(parts)
	}
	pub fn certificate(&self, height: u64) -> Result<Option<CommitCertificate<Context>>> {
		self.get(&[b"c".as_slice(), &height.to_be_bytes()].concat())?
			.map(|raw| self.codec.decode(raw.into()).map_err(Into::into))
			.transpose()
	}
	pub fn save_certificate(&self, cert: &CommitCertificate<Context>) -> Result<()> {
		if let Some(old) = self.certificate(cert.height.0)? {
			ensure!(old.value_id == cert.value_id, "conflicting certificate");
			return Ok(());
		}
		self.put_once(
			&[b"c".as_slice(), &cert.height.0.to_be_bytes()].concat(),
			&self.codec.encode(cert)?,
		)
	}
	pub fn check_wal(&self, path: &Path, committed: u64, validator: Id) -> Result<()> {
		ensure!(path.is_file(), "missing consensus WAL; refusing to sign");
		let mut log = wal::Log::open(path)?;
		ensure!(
			log.sequence() >= committed
				&& log.sequence()
					<= committed.checked_add(1).ok_or_else(|| eyre!("height exhausted"))?,
			"WAL/application height mismatch"
		);
		let mut recorded = BTreeSet::new();
		for entry in log_entries::<Context, _>(&mut log, &self.codec)? {
			let msg = match entry? {
				Input::Vote(v) => Some(SignedConsensusMsg::Vote(v)),
				Input::Proposal(p) => Some(SignedConsensusMsg::Proposal(p)),
				_ => None,
			};
			if let Some(msg) = msg {
				let address = match &msg {
					SignedConsensusMsg::Vote(v) => v.message.address.0,
					SignedConsensusMsg::Proposal(p) => p.message.address.0,
				};
				if address == validator && msg.height().0 > committed {
					recorded.insert(self.codec.encode(&msg)?.to_vec());
				}
			}
		}
		let tx = self.database.begin_read()?;
		let table = tx.open_table(DATA)?;
		let mut journaled = BTreeSet::new();
		let mut last_slot = None;
		for row in table.range(b"s".as_slice()..b"t".as_slice())? {
			let (key, value) = row?;
			let msg: SignedConsensusMsg<Context> =
				self.codec.decode(value.value().to_vec().into())?;
			let address = match &msg {
				SignedConsensusMsg::Vote(v) => v.message.address.0,
				SignedConsensusMsg::Proposal(p) => p.message.address.0,
			};
			ensure!(
				address == validator && key.value().len() == 14 && msg.height().0 <= committed + 1,
				"invalid/stale signing journal"
			);
			let phase = match &msg {
				SignedConsensusMsg::Proposal(_) => 0,
				SignedConsensusMsg::Vote(v)
					if v.message.typ == super::engine::app::types::core::VoteType::Prevote =>
				{
					1
				},
				_ => 2,
			};
			let slot = [
				msg.height().0.to_be_bytes().as_slice(),
				&msg.round().as_u32().ok_or_else(|| eyre!("nil signed round"))?.to_be_bytes(),
				&[phase],
			]
			.concat();
			ensure!(key.value()[1..] == slot, "signing slot mismatch");
			last_slot = Some(slot);
			if msg.height().0 > committed {
				journaled.insert(value.value().to_vec());
				ensure!(
					recorded.contains(value.value()),
					"signed message missing from active WAL; refusing unsafe restart"
				);
			}
		}
		ensure!(journaled == recorded, "active WAL/signing journal mismatch");
		ensure!(
			table.get(b"watermark".as_slice())?.map(|v| v.value().to_vec()) == last_slot,
			"invalid signing high watermark"
		);
		Ok(())
	}
}
