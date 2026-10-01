use rinpqc_node::{
	application::{SignedTransfer, Transfer},
	crypto::{
		self, ConsensusKind, ConsensusMessage, ConsensusSigner, KeyRole, SecretKey,
		TransactionSigner,
	},
	genesis::Genesis,
};

fn fixture() -> serde_json::Value {
	serde_json::from_str(include_str!("fixtures/m1-identities.json")).unwrap()
}
fn genesis() -> Genesis {
	Genesis::from_json(&serde_json::to_vec(&fixture()["genesis"]).unwrap()).unwrap()
}
fn transfer() -> Transfer {
	Transfer::decode(&hex::decode(fixture()["unsigned_transfer_hex"].as_str().unwrap()).unwrap())
		.unwrap()
}

#[test]
fn independent_openssl_signature_and_hash_vectors() {
	let f = fixture();
	let transfer = transfer();
	let key = SecretKey::from_seed(&[1; 32], KeyRole::Transaction);
	let signed = key.sign_transfer(transfer.clone(), &[7; 32]).unwrap();
	assert_eq!(hex::encode(signed.signature), f["signature_hex"]);
	assert_eq!(hex::encode(transfer.id()), f["tx_id"]);
	crypto::verify_transfer(&signed, &[7; 32]).unwrap();
	assert_eq!(SignedTransfer::decode(&signed.encode()).unwrap(), signed);
	// The earlier specification's encoding-only fixture is also preserved.
	let spec: serde_json::Value =
		serde_json::from_str(include_str!("../docs/specs/m1-encoding-vector.json")).unwrap();
	let tx =
		Transfer::decode(&hex::decode(spec["unsigned_transfer_hex"].as_str().unwrap()).unwrap())
			.unwrap();
	assert_eq!(hex::encode(tx.id()), spec["tx_id"]);
	assert_eq!(hex::encode(tx.signing_bytes()), spec["signing_input_hex"]);
}
#[test]
fn tampering_wrong_chain_and_weak_keys_fail() {
	let key = SecretKey::from_seed(&[1; 32], KeyRole::Transaction);
	let signed = key.sign_transfer(transfer(), &[7; 32]).unwrap();
	let mut changed = signed.clone();
	changed.transfer.amount += 1;
	assert!(crypto::verify_transfer(&changed, &[7; 32]).is_err());
	let mut changed_nonce = signed.clone();
	changed_nonce.transfer.nonce += 1;
	assert!(crypto::verify_transfer(&changed_nonce, &[7; 32]).is_err());
	assert!(crypto::verify_transfer(&signed, &[8; 32]).is_err());
	for offset in [0, 31, 32, 63] {
		let mut bad = signed.clone();
		bad.signature[offset] ^= 1;
		assert!(crypto::verify_transfer(&bad, &[7; 32]).is_err());
	}
	let mut noncanonical = signed.clone();
	noncanonical.signature[63] = 255;
	assert!(crypto::verify_transfer(&noncanonical, &[7; 32]).is_err());
	for bad in [[0; 32], [255; 32], {
		let mut x = [0; 32];
		x[0] = 1;
		x
	}] {
		assert!(crypto::public_key(&bad).is_err());
	}
	assert!(SecretKey::from_seed(&[1; 32], KeyRole::Validator)
		.sign_transfer(transfer(), &[7; 32])
		.is_err());
	assert!(SecretKey::from_seed(&[3; 32], KeyRole::Transaction)
		.sign_transfer(transfer(), &[7; 32])
		.is_err());
}
#[test]
fn strict_decoders_reject_truncation_trailing_bytes_and_profiles() {
	let bytes = transfer().encode();
	for n in 0..bytes.len() {
		assert!(Transfer::decode(&bytes[..n]).is_err());
	}
	let mut extra = bytes.clone();
	extra.push(0);
	assert!(Transfer::decode(&extra).is_err());
	for offset in [1, 3] {
		let mut bad = bytes.clone();
		bad[offset] = 2;
		assert!(Transfer::decode(&bad).is_err());
	}
	for n in [0, 116, 179, 181, 1000] {
		assert!(SignedTransfer::decode(&vec![0; n]).is_err());
	}
	let g = genesis().encode().unwrap();
	for n in 0..g.len() {
		assert!(Genesis::decode(&g[..n]).is_err());
	}
	let mut bad = g.clone();
	bad.push(0);
	assert!(Genesis::decode(&bad).is_err());
	let mut bad = g;
	bad[180..184].copy_from_slice(&u32::MAX.to_be_bytes());
	assert!(Genesis::decode(&bad).is_err());
}
#[test]
fn genesis_commitments_match_independent_vectors() {
	let f = fixture();
	let g = genesis();
	assert_eq!(hex::encode(g.encode().unwrap()), f["genesis_hex"]);
	assert_eq!(hex::encode(g.chain_id().unwrap()), f["chain_id"]);
	assert_eq!(hex::encode(g.state_root().unwrap()), f["state_root"]);
	assert_eq!(Genesis::decode(&g.encode().unwrap()).unwrap(), g);
	assert_eq!(Genesis::from_json(&g.to_json().unwrap()).unwrap(), g);
	let mut another = g.clone();
	another.network_nonce[0] ^= 1;
	assert_ne!(another.chain_id().unwrap(), g.chain_id().unwrap());
	assert_eq!(another.state_root().unwrap(), g.state_root().unwrap());
}
#[test]
fn malformed_genesis_is_rejected() {
	let g = genesis();
	let mut bad = g.clone();
	bad.validators[1] = bad.validators[0];
	assert!(bad.validate().is_err());
	let mut bad = g.clone();
	bad.validators.swap(0, 1);
	assert!(bad.validate().is_err());
	let mut bad = g.clone();
	bad.validators[0] = [0; 32];
	assert!(bad.validate().is_err());
	let mut bad = g.clone();
	bad.accounts[0].1 = 0;
	assert!(bad.validate().is_err());
	let mut bad = g.clone();
	bad.accounts[0].1 = u64::MAX;
	assert!(bad.validate().is_err());
	let mut bad = g.clone();
	bad.accounts.push(bad.accounts[0]);
	assert!(bad.validate().is_err());
	let mut bad = g.clone();
	bad.accounts.clear();
	assert!(bad.validate().is_err());
	let mut bad = g.clone();
	bad.accounts = vec![g.accounts[0]; 65_537];
	assert!(bad.validate().is_err());
	for limit in [0, 143, 1_048_577] {
		let mut bad = g.clone();
		bad.max_block_bytes = limit;
		assert!(bad.validate().is_err());
	}
	for interval in [0, 60_001] {
		let mut bad = g.clone();
		bad.target_interval_ms = interval;
		assert!(bad.validate().is_err());
	}
	let mut f = fixture()["genesis"].clone();
	f["accounts"][0]["balance"] = "001".into();
	assert!(Genesis::from_json(&serde_json::to_vec(&f).unwrap()).is_err());
	let mut f = fixture()["genesis"].clone();
	f["extra"] = true.into();
	assert!(Genesis::from_json(&serde_json::to_vec(&f).unwrap()).is_err());
	let mut f = fixture()["genesis"].clone();
	f["suite"] = 2.into();
	assert!(Genesis::from_json(&serde_json::to_vec(&f).unwrap()).is_err());
}
#[test]
fn consensus_context_and_key_roles_cannot_be_replayed() {
	let key = SecretKey::from_seed(&[11; 32], KeyRole::Validator);
	let msg = ConsensusMessage {
		chain_id: [7; 32],
		height: 1,
		round: 0,
		kind: ConsensusKind::Prevote,
		value: Some([3; 32]),
	};
	let sig = key.sign_consensus(&msg, &[7; 32]).unwrap();
	crypto::verify_consensus(&key.public_key(), &msg, &[7; 32], &sig).unwrap();
	let mut variants = vec![];
	let mut m = msg.clone();
	m.kind = ConsensusKind::Precommit;
	variants.push(m);
	let mut m = msg.clone();
	m.kind = ConsensusKind::Proposal;
	variants.push(m);
	let mut m = msg.clone();
	m.height = 2;
	variants.push(m);
	let mut m = msg.clone();
	m.round = 1;
	variants.push(m);
	let mut m = msg.clone();
	m.value = None;
	variants.push(m);
	let mut m = msg.clone();
	m.chain_id = [8; 32];
	variants.push(m);
	for m in variants {
		assert!(crypto::verify_consensus(&key.public_key(), &m, &[7; 32], &sig).is_err());
	}
	assert!(crypto::verify_signature(&key.public_key(), &transfer().signing_bytes(), &sig).is_err());
	assert!(SecretKey::from_seed(&[11; 32], KeyRole::Network)
		.sign_consensus(&msg, &[7; 32])
		.is_err());
	let mut m = msg.clone();
	m.height = 0;
	assert!(m.signing_bytes().is_err());
	let mut m = msg.clone();
	m.round = u32::MAX;
	assert!(m.signing_bytes().is_err());
	let mut m = msg;
	m.kind = ConsensusKind::Proposal;
	m.value = None;
	assert!(m.signing_bytes().is_err());
}
