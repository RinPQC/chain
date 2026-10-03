use super::{genesis::Genesis, wire::*, *};
fn fixture() -> (Genesis, pq::SecretKey, pq::SecretKey) {
	let account = pq::SecretKey::generate(KeyRole::Transaction).unwrap();
	let network = pq::SecretKey::generate(KeyRole::Network).unwrap();
	let mut validators =
		std::array::from_fn(|_| pq::SecretKey::generate(KeyRole::Validator).unwrap().public_key());
	validators.sort_by_key(|k| ValidatorId::from_key(k).unwrap());
	let g = Genesis {
		network_nonce: [4; 32],
		max_block_bytes: MAX_BLOCK_BYTES as u32,
		max_transactions: 4096,
		target_interval_ms: 5000,
		validators,
		accounts: vec![(AccountId::from_key(&account.public_key()).unwrap(), 100)],
	};
	(g, account, network)
}
fn envelope(chain: ChainId, key: &pq::SecretKey) -> Envelope {
	let transfer = Transfer {
		chain,
		sender: AccountId::from_key(&key.public_key()).unwrap(),
		recipient: AccountId([9; 48]),
		amount: 1,
		nonce: 0,
	};
	let signature = key.sign(pq::Domain::Transfer, &transfer.encode()).unwrap();
	Envelope { transfer, sender_key: key.public_key(), signature }
}
#[test]
fn genesis_binds_full_keys_parameters_and_initial_state() {
	let (g, key, _) = fixture();
	let bytes = g.encode().unwrap();
	let chain = g.chain_id().unwrap();
	assert_eq!(Genesis::decode_for_chain(&bytes, chain).unwrap(), g);
	assert_eq!(g.state_root().unwrap(), state_root(&[(g.accounts[0].0, 100, 0)]).unwrap());
	let mut bad = g.clone();
	bad.network_nonce[0] ^= 1;
	assert_ne!(bad.chain_id().unwrap(), chain);
	bad = g.clone();
	bad.max_transactions -= 1;
	assert_ne!(bad.chain_id().unwrap(), chain);
	bad = g.clone();
	bad.validators.swap(0, 1);
	assert!(bad.encode().is_err());
	bad = g.clone();
	bad.validators[0] = key.public_key();
	assert!(bad.encode().is_err());
	bad = g.clone();
	bad.accounts.push(bad.accounts[0]);
	assert!(bad.encode().is_err());
	bad = g.clone();
	bad.accounts[0].1 = 0;
	assert!(bad.encode().is_err());
	bad = g.clone();
	bad.accounts = vec![(AccountId([0; 48]), u64::MAX), (AccountId([1; 48]), 1)];
	assert!(bad.encode().is_err());
	let mut bytes = bytes;
	let pos = genesis::GENESIS_FIXED_LEN - 4;
	bytes[pos..pos + 4].copy_from_slice(&u32::MAX.to_be_bytes());
	assert!(Genesis::decode(&bytes).is_err());
}
#[test]
fn envelopes_bind_keys_but_ids_do_not_depend_on_signature_randomness() {
	let (g, key, network) = fixture();
	let chain = g.chain_id().unwrap();
	let a = envelope(chain, &key);
	let b = envelope(chain, &key);
	assert!(a.signature != b.signature);
	assert_eq!(a.transfer.id(), b.transfer.id());
	assert!(Envelope::decode(&a.encode().unwrap(), chain).unwrap() == a);
	let mut bad = a.clone();
	bad.sender_key = pq::SecretKey::generate(KeyRole::Transaction).unwrap().public_key();
	assert!(bad.encode().is_err());
	assert!(AccountId::from_key(&network.public_key()).is_err());
	assert!(ValidatorId::from_key(&key.public_key()).is_err());
	let mut raw = a.encode().unwrap();
	let offset = PREFIX_LEN + TRANSFER_LEN;
	raw[offset..offset + pq::PUBLIC_KEY_BYTES].copy_from_slice(&bad.sender_key.to_bytes());
	assert!(Envelope::decode(&raw, chain).is_err());
	// Context/profile/chain are authenticated by the subsequent authorization layer.
	key.public_key().verify(pq::Domain::Transfer, &a.transfer.encode(), &a.signature).unwrap();
}
#[test]
fn blocks_are_bounded_and_commit_to_exact_authorizations() {
	let (g, key, _) = fixture();
	let chain = g.chain_id().unwrap();
	let transfers = vec![envelope(chain, &key)];
	let block = Block {
		header: Header {
			chain,
			height: 1,
			parent: genesis_block_id(chain),
			transactions_root: transactions_root(&transfers).unwrap(),
			state_root: g.state_root().unwrap(),
		},
		transfers,
	};
	let bytes = block.encode().unwrap();
	assert_eq!(bytes.len(), MIN_BLOCK_LEN + ENVELOPE_LEN);
	assert!(Block::decode(&bytes, chain, g.max_block_bytes, g.max_transactions).unwrap() == block);
	assert!(Block::decode(&bytes, chain, g.max_block_bytes, 0).is_err());
	assert!(Block::decode(&bytes, chain, (bytes.len() - 1) as u32, g.max_transactions).is_err());
	let mut bad = bytes.clone();
	bad[HEADER_LEN..MIN_BLOCK_LEN].copy_from_slice(&u32::MAX.to_be_bytes());
	assert!(Block::decode(&bad, chain, g.max_block_bytes, g.max_transactions).is_err());
	let mut other = block.clone();
	other.transfers[0] = envelope(chain, &key);
	assert_eq!(other.transfers[0].transfer.id(), block.transfers[0].transfer.id());
	assert!(other.encode().is_err()); // Existing header cannot cover a substituted authorization.
	other.header.transactions_root = transactions_root(&other.transfers).unwrap();
	assert_ne!(other.header.id().unwrap(), block.header.id().unwrap());
}
#[test]
fn certificates_have_unique_sorted_bounded_signers_and_exact_signatures() {
	let (g, _, _) = fixture();
	let chain = g.chain_id().unwrap();
	let signing = pq::SecretKey::generate(KeyRole::Validator).unwrap();
	// Shape-only fixture: these are deliberately not signatures by the listed members.
	let signature = signing.sign(pq::Domain::Precommit, b"shape only").unwrap();
	let endorsements = g.validators[..3]
		.iter()
		.map(|k| Endorsement {
			validator: ValidatorId::from_key(k).unwrap(),
			signature: signature.clone(),
		})
		.collect();
	let cert = Certificate { chain, height: 1, round: 0, value: BlockId([8; 48]), endorsements };
	let bytes = cert.encode().unwrap();
	assert!(Certificate::decode(&bytes, chain).unwrap() == cert);
	let mut bad = cert.clone();
	bad.endorsements.swap(0, 1);
	assert!(bad.encode().is_err());
	bad = cert.clone();
	bad.endorsements[1] = bad.endorsements[0].clone();
	assert!(bad.encode().is_err());
	bad = cert.clone();
	bad.endorsements.pop();
	assert!(bad.encode().is_err());
	bad = cert;
	bad.round = u32::MAX;
	assert!(bad.encode().is_err());
}
#[test]
fn every_wire_parser_rejects_m1_wrong_profile_truncation_and_trailing_bytes() {
	let (g, key, _) = fixture();
	let chain = g.chain_id().unwrap();
	let tx = envelope(chain, &key);
	let header = Header {
		chain,
		height: 1,
		parent: genesis_block_id(chain),
		transactions_root: transactions_root(&[]).unwrap(),
		state_root: g.state_root().unwrap(),
	};
	let block = Block { header: header.clone(), transfers: vec![] };
	let sig = key.sign(pq::Domain::Transfer, b"fixture").unwrap();
	let cert = Certificate {
		chain,
		height: 1,
		round: 0,
		value: header.id().unwrap(),
		endorsements: (1..=3)
			.map(|i| Endorsement { validator: ValidatorId([i; 48]), signature: sig.clone() })
			.collect(),
	};
	let samples = [
		g.encode().unwrap(),
		tx.transfer.encode(),
		tx.encode().unwrap(),
		header.encode().unwrap(),
		cert.encode().unwrap(),
		block.encode().unwrap(),
	];
	let check = |index: usize, b: &[u8], c: ChainId| -> Result<()> {
		match index {
			0 => Genesis::decode_for_chain(b, c).map(|_| ()),
			1 => Transfer::decode(b, c).map(|_| ()),
			2 => Envelope::decode(b, c).map(|_| ()),
			3 => Header::decode(b, c).map(|_| ()),
			4 => Certificate::decode(b, c).map(|_| ()),
			_ => Block::decode(b, c, g.max_block_bytes, g.max_transactions).map(|_| ()),
		}
	};
	for (index, bytes) in samples.iter().enumerate() {
		check(index, bytes, chain).unwrap();
		assert!(check(index, bytes, ChainId([0; 48])).is_err());
		for position in [0, 4, 6, 8] {
			let mut bad = bytes.clone();
			bad[position] ^= 0xff;
			assert!(check(index, &bad, chain).is_err());
		}
		let mut bad = bytes.clone();
		bad.push(0);
		assert!(check(index, &bad, chain).is_err());
		for len in [0, 1, PREFIX_LEN - 1, bytes.len() - 1] {
			assert!(check(index, &bytes[..len], chain).is_err());
		}
		assert!(check(index, &[0; 180], chain).is_err());
	}
}
#[cfg(unix)]
#[test]
fn fresh_directories_refuse_m1_existing_partial_and_mixed_state() {
	use std::{fs, os::unix::fs::symlink};
	let (g, _, network) = fixture();
	let root = tempfile::tempdir().unwrap();
	let path = root.path().join("m2");
	storage::initialize(&path, &g, &g.validators[0], &network.public_key()).unwrap();
	storage::check(&path, &g, &g.validators[0], &network.public_key()).unwrap();
	assert!(storage::initialize(&path, &g, &g.validators[0], &network.public_key()).is_err());
	assert!(storage::check(&path, &g, &g.validators[1], &network.public_key()).is_err());
	let original = fs::read(path.join("genesis.bin")).unwrap();
	fs::write(path.join("application.redb"), b"M1 history").unwrap();
	assert!(storage::check(&path, &g, &g.validators[0], &network.public_key()).is_err());
	assert_eq!(fs::read(path.join("genesis.bin")).unwrap(), original);
	assert_eq!(fs::read(path.join("application.redb")).unwrap(), b"M1 history");
	let m1 = root.path().join("m1");
	fs::create_dir(&m1).unwrap();
	fs::write(m1.join("identity.bin"), b"M1").unwrap();
	assert!(storage::initialize(&m1, &g, &g.validators[0], &network.public_key()).is_err());
	let link = root.path().join("link");
	symlink(&m1, &link).unwrap();
	assert!(storage::initialize(&link, &g, &g.validators[0], &network.public_key()).is_err());
	let partial = root.path().join("partial");
	storage::initialize(&partial, &g, &g.validators[0], &network.public_key()).unwrap();
	fs::remove_file(partial.join("profile.bin")).unwrap();
	assert!(storage::check(&partial, &g, &g.validators[0], &network.public_key()).is_err());
	assert!(storage::initialize(&partial, &g, &g.validators[0], &network.public_key()).is_err());
}

#[test]
fn independent_python_canonical_vectors_and_identifier_text() {
	let value: serde_json::Value =
		serde_json::from_str(include_str!("../../tests/fixtures/m2/canonical.json")).unwrap();
	let bytes = |name: &str| hex::decode(value[name].as_str().unwrap()).unwrap();
	let chain = ChainId::from_hex(value["chain_id"].as_str().unwrap()).unwrap();
	let g = Genesis::decode_for_chain(&bytes("genesis"), chain).unwrap();
	assert_eq!(g.encode().unwrap(), bytes("genesis"));
	assert_eq!(g.state_root().unwrap().to_hex(), value["state_root"]);
	let public =
		pq::PublicKey::from_bytes(pq::SUITE, KeyRole::Transaction, &bytes("public_key")).unwrap();
	assert_eq!(AccountId::from_key(&public).unwrap().to_hex(), value["account_id"]);
	let public =
		pq::PublicKey::from_bytes(pq::SUITE, KeyRole::Validator, &bytes("public_key")).unwrap();
	assert_eq!(ValidatorId::from_key(&public).unwrap().to_hex(), value["validator_id"]);
	let tx = Transfer::decode(&bytes("transfer"), chain).unwrap();
	assert_eq!(tx.encode(), bytes("transfer"));
	assert_eq!(tx.id().to_hex(), value["transaction_id"]);
	let envelope = Envelope::decode(&bytes("envelope"), chain).unwrap();
	assert_eq!(envelope.encode().unwrap(), bytes("envelope"));
	// Fixtures establish canonical structure, never valid payment authorization.
	assert!(envelope
		.sender_key
		.verify(pq::Domain::Transfer, &tx.encode(), &envelope.signature)
		.is_err());
	let header = Header::decode(&bytes("header"), chain).unwrap();
	assert_eq!(header.encode().unwrap(), bytes("header"));
	assert_eq!(header.id().unwrap().to_hex(), value["block_id"]);
	assert_eq!(transactions_root(&[envelope]).unwrap().to_hex(), value["transactions_root"]);
	let block =
		Block::decode(&bytes("block"), chain, g.max_block_bytes, g.max_transactions).unwrap();
	assert_eq!(block.encode().unwrap(), bytes("block"));
	let cert = Certificate::decode(&bytes("certificate"), chain).unwrap();
	assert_eq!(cert.encode().unwrap(), bytes("certificate"));
	assert!(ChainId::from_hex(&chain.to_hex().to_uppercase()).is_err());
	assert!(AccountId::from_hex(&"0".repeat(64)).is_err());
	assert!(AccountId::from_hex(&format!(" {}", chain.to_hex())).is_err());
	assert_eq!(ChainId::from_hex(&chain.to_hex()).unwrap(), chain);
	assert_ne!(commitment(b"ACCOUNT", b"x"), commitment(b"VALIDATOR", b"x"));
	assert_ne!(commitment(b"A", b"BC"), commitment(b"AB", b"C"));
}

#[test]
fn valid_m1_genesis_and_payment_bytes_are_not_m2_and_fresh_nonce_changes_chain() {
	let keys: [_; 4] = std::array::from_fn(|i| {
		crate::crypto::SecretKey::from_seed(&[i as u8 + 1; 32], KeyRole::Validator).public_key()
	});
	let mut validators = keys;
	validators.sort();
	let old = crate::genesis::Genesis {
		network_nonce: [1; 32],
		max_block_bytes: 1048576,
		max_transactions: 4096,
		target_interval_ms: 5000,
		validators,
		accounts: vec![(validators[0], 100)],
	};
	let old_bytes = old.encode().unwrap();
	assert!(Genesis::decode(&old_bytes).is_err());
	let (g, key, _) = fixture();
	let chain = g.chain_id().unwrap();
	let old_tx = crate::application::Transfer {
		chain_id: old.chain_id().unwrap(),
		sender: validators[0],
		recipient: validators[1],
		amount: 1,
		nonce: 0,
	};
	assert!(Transfer::decode(&old_tx.encode(), chain).is_err());
	assert!(crate::genesis::Genesis::decode(&g.encode().unwrap()).is_err());
	assert!(crate::application::Transfer::decode(&envelope(chain, &key).transfer.encode()).is_err());
	let fresh = Genesis::fresh(
		g.validators.clone(),
		g.accounts.clone(),
		g.max_block_bytes,
		g.max_transactions,
		g.target_interval_ms,
	)
	.unwrap();
	assert_ne!(fresh.chain_id().unwrap(), chain);
	assert_eq!(fresh.state_root().unwrap(), g.state_root().unwrap());
}
