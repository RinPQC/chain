#![cfg(unix)]
use rinpqc_node::{
	crypto::{KeyRole, SecretKey},
	genesis::Genesis,
	infrastructure::{load_key, save_key, NodeConfig},
};
use std::{
	fs,
	os::unix::fs::{symlink, PermissionsExt},
	process::Command,
};

fn setup() -> (tempfile::TempDir, Genesis, String) {
	let tmp = tempfile::tempdir().unwrap();
	let f: serde_json::Value =
		serde_json::from_str(include_str!("fixtures/m1-identities.json")).unwrap();
	let g = Genesis::from_json(&serde_json::to_vec(&f["genesis"]).unwrap()).unwrap();
	fs::write(tmp.path().join("genesis.json"), g.to_json().unwrap()).unwrap();
	save_key(
		&tmp.path().join("validator.key"),
		&SecretKey::from_seed(&[11; 32], KeyRole::Validator),
	)
	.unwrap();
	save_key(&tmp.path().join("network.key"), &SecretKey::from_seed(&[21; 32], KeyRole::Network))
		.unwrap();
	let config=format!("version = 1\ngenesis = 'genesis.json'\nexpected_chain_id = '{}'\nvalidator_key = 'validator.key'\nnetwork_key = 'network.key'\ndata_dir = 'node-data'\nlisten = '127.0.0.1:31001'\npeers = []\n",hex::encode(g.chain_id().unwrap()));
	fs::write(tmp.path().join("node.toml"), &config).unwrap();
	(tmp, g, config)
}
#[test]
fn keys_are_private_bounded_role_bound_and_never_overwritten() {
	let tmp = tempfile::tempdir().unwrap();
	let path = tmp.path().join("key");
	let key = SecretKey::generate(KeyRole::Validator).unwrap();
	save_key(&path, &key).unwrap();
	assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
	assert_eq!(load_key(&path, KeyRole::Validator).unwrap().public_key(), key.public_key());
	assert!(save_key(&path, &key).is_err());
	assert!(load_key(&path, KeyRole::Transaction).is_err());
	let link = tmp.path().join("link");
	symlink(&path, &link).unwrap();
	assert!(load_key(&link, KeyRole::Validator).is_err());
	fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
	assert!(load_key(&path, KeyRole::Validator).is_err());
	fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
	let original = fs::read(&path).unwrap();
	for bytes in [original[..42].to_vec(), [original.clone(), vec![0]].concat(), vec![0; 43]] {
		fs::write(&path, bytes).unwrap();
		assert!(load_key(&path, KeyRole::Validator).is_err());
	}
}
#[test]
fn genesis_and_directory_binding_fail_closed() {
	let (tmp, g, text) = setup();
	let path = tmp.path().join("node.toml");
	let config = NodeConfig::load(&path).unwrap();
	assert!(!config.data_dir.exists());
	config.initialize().unwrap();
	config.initialize().unwrap();
	assert_eq!(fs::metadata(&config.data_dir).unwrap().permissions().mode() & 0o777, 0o700);
	let wrong = text.replace(&hex::encode(g.chain_id().unwrap()), &"00".repeat(32));
	fs::write(&path, wrong).unwrap();
	assert!(NodeConfig::load(&path).is_err());
	fs::write(&path, &text).unwrap();
	save_key(&tmp.path().join("other.key"), &SecretKey::from_seed(&[12; 32], KeyRole::Validator))
		.unwrap();
	fs::write(&path, text.replace("validator.key", "other.key")).unwrap();
	assert!(NodeConfig::load(&path).is_err());
	fs::write(&path, &text).unwrap();
	fs::remove_file(config.data_dir.join("identity.bin")).unwrap();
	assert!(NodeConfig::load(&path).is_err());
}
#[test]
fn peer_and_identity_misconfiguration_is_rejected() {
	let (tmp, _, text) = setup();
	let path = tmp.path().join("node.toml");
	let peer = hex::encode(SecretKey::from_seed(&[22; 32], KeyRole::Network).public_key());
	let prefix = text.replace("peers = []\n", "");
	let record = format!("[[peers]]\naddress = '127.0.0.1:31002'\npublic_key = '{peer}'\n");
	fs::write(&path, format!("{prefix}{record}")).unwrap();
	assert_eq!(NodeConfig::load(&path).unwrap().peers.len(), 1);
	fs::write(&path, format!("{prefix}{record}{record}")).unwrap();
	assert!(NodeConfig::load(&path).is_err());
	fs::write(&path, text.replace("network.key", "validator.key")).unwrap();
	assert!(NodeConfig::load(&path).is_err());
	fs::write(&path, text.replace("127.0.0.1:31001", "127.0.0.1:0")).unwrap();
	assert!(NodeConfig::load(&path).is_err());
	fs::write(&path, &text).unwrap();
	symlink(tmp.path(), tmp.path().join("node-data")).unwrap();
	assert!(NodeConfig::load(&path).is_err());
}
#[test]
fn cli_generates_only_public_output_and_validates_local_setup() {
	let (tmp, g, _) = setup();
	let output = Command::new(env!("CARGO_BIN_EXE_rinpqc-node"))
		.args(["keygen", "transaction"])
		.arg(tmp.path().join("payment.key"))
		.output()
		.unwrap();
	assert!(output.status.success());
	assert!(output.stderr.is_empty());
	let public = String::from_utf8(output.stdout).unwrap();
	assert_eq!(public.trim().len(), 64);
	assert_eq!(
		public.trim(),
		hex::encode(
			load_key(&tmp.path().join("payment.key"), KeyRole::Transaction).unwrap().public_key()
		)
	);
	for cmd in ["config-check", "init", "config-check"] {
		let output = Command::new(env!("CARGO_BIN_EXE_rinpqc-node"))
			.arg(cmd)
			.arg(tmp.path().join("node.toml"))
			.output()
			.unwrap();
		assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
		assert!(
			String::from_utf8_lossy(&output.stdout).contains(&hex::encode(g.chain_id().unwrap()))
		);
	}
}

#[test]
fn rpc_is_opt_in_and_restricted_to_a_separate_loopback_port() {
	let (tmp, _, text) = setup();
	let path = tmp.path().join("node.toml");
	assert!(NodeConfig::load(&path).unwrap().rpc_listen.is_none());
	for address in ["0.0.0.0:32000", "192.0.2.1:32000", "127.0.0.1:0", "127.0.0.1:31001"] {
		fs::write(&path, format!("{text}rpc_listen = '{address}'\n")).unwrap();
		assert!(NodeConfig::load(&path).is_err(), "accepted {address}");
	}
	fs::write(&path, format!("{text}rpc_listen = '127.0.0.1:32000'\n")).unwrap();
	assert_eq!(NodeConfig::load(&path).unwrap().rpc_listen.unwrap().port(), 32000);
}
