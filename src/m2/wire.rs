//! Structural decoding is not authorization, execution or proof of finality.
use super::*;

pub const TRANSFER_LEN: usize = PREFIX_LEN + 48 * 3 + 8 * 2;
pub const ENVELOPE_LEN: usize =
	PREFIX_LEN + TRANSFER_LEN + pq::PUBLIC_KEY_BYTES + pq::SIGNATURE_BYTES;
pub const HEADER_LEN: usize = PREFIX_LEN + 48 * 4 + 8;
pub const MIN_BLOCK_LEN: usize = HEADER_LEN + 4;
const CERT_FIXED_LEN: usize = PREFIX_LEN + 48 + 8 + 4 + 48 + 1;
const ENDORSEMENT_LEN: usize = 48 + pq::SIGNATURE_BYTES;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
	pub chain: ChainId,
	pub sender: AccountId,
	pub recipient: AccountId,
	pub amount: u64,
	pub nonce: u64,
}
impl Transfer {
	pub fn encode(&self) -> Vec<u8> {
		let mut out = prefix(2);
		out.extend(self.chain.0);
		out.extend(self.sender.0);
		out.extend(self.recipient.0);
		out.extend(self.amount.to_be_bytes());
		out.extend(self.nonce.to_be_bytes());
		out
	}
	pub fn decode(bytes: &[u8], chain: ChainId) -> Result<Self> {
		if bytes.len() != TRANSFER_LEN {
			return Err(Error::Encoding);
		}
		let mut r = Reader::new(bytes, 2, TRANSFER_LEN)?;
		r.chain(chain)?;
		let value = Self {
			chain,
			sender: AccountId(r.array()?),
			recipient: AccountId(r.array()?),
			amount: r.u64()?,
			nonce: r.u64()?,
		};
		r.finish()?;
		Ok(value)
	}
	/// Signature-independent logical payment ID. Every envelope still requires authorization.
	pub fn id(&self) -> TransactionId {
		TransactionId(commitment(b"TXID", &self.encode()))
	}
}
#[derive(Clone, PartialEq, Eq)]
pub struct Envelope {
	pub transfer: Transfer,
	pub sender_key: pq::PublicKey,
	pub signature: pq::Signature,
}
impl Envelope {
	fn binding(&self) -> Result<()> {
		if AccountId::from_key(&self.sender_key)? != self.transfer.sender {
			return Err(Error::Key);
		}
		Ok(())
	}
	pub fn encode(&self) -> Result<Vec<u8>> {
		self.binding()?;
		let mut out = prefix(3);
		out.extend(self.transfer.encode());
		out.extend(self.sender_key.to_bytes());
		out.extend(self.signature.as_bytes());
		Ok(out)
	}
	pub fn decode(bytes: &[u8], chain: ChainId) -> Result<Self> {
		if bytes.len() != ENVELOPE_LEN {
			return Err(Error::Encoding);
		}
		let mut r = Reader::new(bytes, 3, ENVELOPE_LEN)?;
		let transfer = Transfer::decode(r.take(TRANSFER_LEN)?, chain)?;
		let sender_key = pq::PublicKey::from_bytes(
			pq::SUITE,
			KeyRole::Transaction,
			r.take(pq::PUBLIC_KEY_BYTES)?,
		)?;
		let signature = pq::Signature::from_bytes(pq::SUITE, r.take(pq::SIGNATURE_BYTES)?)?;
		r.finish()?;
		let value = Self { transfer, sender_key, signature };
		value.binding()?;
		Ok(value)
	}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
	pub chain: ChainId,
	pub height: u64,
	pub parent: BlockId,
	pub transactions_root: TransactionsRoot,
	pub state_root: StateRoot,
}
impl Header {
	pub fn encode(&self) -> Result<Vec<u8>> {
		if self.height == 0 {
			return Err(Error::Config("block height must be nonzero"));
		}
		let mut out = prefix(4);
		out.extend(self.chain.0);
		out.extend(self.height.to_be_bytes());
		out.extend(self.parent.0);
		out.extend(self.transactions_root.0);
		out.extend(self.state_root.0);
		Ok(out)
	}
	pub fn decode(bytes: &[u8], chain: ChainId) -> Result<Self> {
		if bytes.len() != HEADER_LEN {
			return Err(Error::Encoding);
		}
		let mut r = Reader::new(bytes, 4, HEADER_LEN)?;
		r.chain(chain)?;
		let value = Self {
			chain,
			height: r.u64()?,
			parent: BlockId(r.array()?),
			transactions_root: TransactionsRoot(r.array()?),
			state_root: StateRoot(r.array()?),
		};
		r.finish()?;
		if value.height == 0 {
			return Err(Error::Encoding);
		}
		Ok(value)
	}
	pub fn id(&self) -> Result<BlockId> {
		Ok(BlockId(commitment(b"BLOCK", &self.encode()?)))
	}
}
/// The height-zero anchor is the chain commitment, explicitly converted into the block-ID type.
pub fn genesis_block_id(chain: ChainId) -> BlockId {
	BlockId(chain.0)
}

#[derive(Clone, PartialEq, Eq)]
pub struct Block {
	pub header: Header,
	pub transfers: Vec<Envelope>,
}
fn block_len(count: usize, max_bytes: usize, max_transactions: usize) -> Result<usize> {
	if count > MAX_TRANSACTIONS || count > max_transactions {
		return Err(Error::Encoding);
	}
	let len = MIN_BLOCK_LEN + count * ENVELOPE_LEN;
	if len > MAX_BLOCK_BYTES || len > max_bytes {
		return Err(Error::Encoding);
	}
	Ok(len)
}
fn body(transfers: &[Envelope]) -> Result<Vec<u8>> {
	block_len(transfers.len(), MAX_BLOCK_BYTES, MAX_TRANSACTIONS)?;
	let mut out = Vec::with_capacity(4 + transfers.len() * ENVELOPE_LEN);
	out.extend((transfers.len() as u32).to_be_bytes());
	for tx in transfers {
		out.extend(tx.encode()?);
	}
	Ok(out)
}
pub fn transactions_root(transfers: &[Envelope]) -> Result<TransactionsRoot> {
	Ok(TransactionsRoot(commitment(b"TXLIST", &body(transfers)?)))
}
impl Block {
	pub fn encode(&self) -> Result<Vec<u8>> {
		if self.transfers.iter().any(|t| t.transfer.chain != self.header.chain)
			|| transactions_root(&self.transfers)? != self.header.transactions_root
		{
			return Err(Error::Encoding);
		}
		let mut out = self.header.encode()?;
		out.extend(body(&self.transfers)?);
		Ok(out)
	}
	pub fn decode(
		bytes: &[u8],
		chain: ChainId,
		max_bytes: u32,
		max_transactions: u32,
	) -> Result<Self> {
		if bytes.len() < MIN_BLOCK_LEN
			|| bytes.len() > MAX_BLOCK_BYTES
			|| bytes.len() > max_bytes as usize
		{
			return Err(Error::Encoding);
		}
		let header = Header::decode(&bytes[..HEADER_LEN], chain)?;
		let count = u32::from_be_bytes(
			bytes[HEADER_LEN..MIN_BLOCK_LEN].try_into().map_err(|_| Error::Encoding)?,
		) as usize;
		if bytes.len() != block_len(count, max_bytes as usize, max_transactions as usize)? {
			return Err(Error::Encoding);
		}
		if TransactionsRoot(commitment(b"TXLIST", &bytes[HEADER_LEN..])) != header.transactions_root
		{
			return Err(Error::Encoding);
		}
		let transfers = bytes[MIN_BLOCK_LEN..]
			.chunks_exact(ENVELOPE_LEN)
			.map(|b| Envelope::decode(b, chain))
			.collect::<Result<_>>()?;
		Ok(Self { header, transfers })
	}
}

#[derive(Clone, PartialEq, Eq)]
pub struct Endorsement {
	pub validator: ValidatorId,
	pub signature: pq::Signature,
}
/// A canonical candidate certificate, not a verified finality certificate. #39 supplies
/// verification.
#[derive(Clone, PartialEq, Eq)]
pub struct Certificate {
	pub chain: ChainId,
	pub height: u64,
	pub round: u32,
	pub value: BlockId,
	pub endorsements: Vec<Endorsement>,
}
impl Certificate {
	fn structure(&self) -> Result<()> {
		if self.height == 0
			|| self.round > i32::MAX as u32
			|| !(3..=4).contains(&self.endorsements.len())
			|| self.endorsements.windows(2).any(|w| w[0].validator >= w[1].validator)
		{
			return Err(Error::Encoding);
		}
		Ok(())
	}
	pub fn encode(&self) -> Result<Vec<u8>> {
		self.structure()?;
		let mut out = prefix(5);
		out.extend(self.chain.0);
		out.extend(self.height.to_be_bytes());
		out.extend(self.round.to_be_bytes());
		out.extend(self.value.0);
		out.push(self.endorsements.len() as u8);
		for vote in &self.endorsements {
			out.extend(vote.validator.0);
			out.extend(vote.signature.as_bytes());
		}
		Ok(out)
	}
	pub fn decode(bytes: &[u8], chain: ChainId) -> Result<Self> {
		let mut r = Reader::new(bytes, 5, CERT_FIXED_LEN + 4 * ENDORSEMENT_LEN)?;
		r.chain(chain)?;
		let height = r.u64()?;
		let round = r.u32()?;
		let value = BlockId(r.array()?);
		let count = r.u8()? as usize;
		if height == 0
			|| round > i32::MAX as u32
			|| !(3..=4).contains(&count)
			|| bytes.len() != CERT_FIXED_LEN + count * ENDORSEMENT_LEN
		{
			return Err(Error::Encoding);
		}
		let mut endorsements = Vec::with_capacity(count);
		for _ in 0..count {
			endorsements.push(Endorsement {
				validator: ValidatorId(r.array()?),
				signature: pq::Signature::from_bytes(pq::SUITE, r.take(pq::SIGNATURE_BYTES)?)?,
			});
		}
		r.finish()?;
		let value = Self { chain, height, round, value, endorsements };
		value.structure()?;
		Ok(value)
	}
}
