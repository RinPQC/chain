//! Real subprocesses and loopback transport, not mocked consensus.
#![cfg(unix)]
use rinpqc_node::{
	application::{SignedTransfer, Transfer},
	crypto::{KeyRole, SecretKey, TransactionSigner},
	genesis::Genesis,
	infrastructure::save_key,
	storage::Store,
};
use std::{
	fs,
	net::TcpListener,
	path::PathBuf,
	process::{Child, Command, Stdio},
	thread,
	time::{Duration, Instant},
};
struct Network {
	dir: tempfile::TempDir,
	genesis: Genesis,
	tx: SignedTransfer,
	first: usize,
	submitter: usize,
}
struct Running {
	children: Vec<(usize, Child)>,
	dir: PathBuf,
}
impl Drop for Running {
	fn drop(&mut self) {
		for (_, child) in &mut self.children {
			let _ = child.kill();
			let _ = child.wait();
		}
	}
}
impl Network {
	fn new() -> Self {
		let dir = tempfile::tempdir().unwrap();
		let validators: Vec<_> =
			(0..4).map(|_| SecretKey::generate(KeyRole::Validator).unwrap()).collect();
		let networks: Vec<_> =
			(0..4).map(|_| SecretKey::generate(KeyRole::Network).unwrap()).collect();
		let sender = SecretKey::generate(KeyRole::Transaction).unwrap();
		let recipient = SecretKey::generate(KeyRole::Transaction).unwrap();
		let mut ids: [_; 4] =
			validators.iter().map(|k| k.public_key()).collect::<Vec<_>>().try_into().unwrap();
		ids.sort();
		let first = validators.iter().position(|k| k.public_key() == ids[0]).unwrap();
		let submitter = validators.iter().position(|k| k.public_key() == ids[3]).unwrap();
		let genesis = Genesis {
			network_nonce: sender.public_key(),
			max_block_bytes: 1_048_576,
			max_transactions: 4096,
			target_interval_ms: 5000,
			validators: ids,
			accounts: vec![(sender.public_key(), 100)],
		};
		let tx = sender
			.sign_transfer(
				Transfer {
					chain_id: genesis.chain_id().unwrap(),
					sender: sender.public_key(),
					recipient: recipient.public_key(),
					amount: 30,
					nonce: 0,
				},
				&genesis.chain_id().unwrap(),
			)
			.unwrap();
		fs::write(dir.path().join("genesis.json"), genesis.to_json().unwrap()).unwrap();
		fs::write(dir.path().join("payments.bin"), tx.encode()).unwrap();
		let listeners: Vec<_> = (0..4).map(|_| TcpListener::bind("127.0.0.1:0").unwrap()).collect();
		let ports: Vec<_> = listeners.iter().map(|l| l.local_addr().unwrap()).collect();
		for i in 0..4 {
			save_key(&dir.path().join(format!("validator{i}.key")), &validators[i]).unwrap();
			save_key(&dir.path().join(format!("network{i}.key")), &networks[i]).unwrap();
			let mut text=format!("version=1\ngenesis='genesis.json'\nexpected_chain_id='{}'\nvalidator_key='validator{i}.key'\nnetwork_key='network{i}.key'\ndata_dir='node{i}'\nlisten='{}'\n",hex::encode(genesis.chain_id().unwrap()),ports[i]);
			for j in 0..4 {
				if i != j {
					text += &format!(
						"\n[[peers]]\naddress='{}'\npublic_key='{}'\n",
						ports[j],
						hex::encode(networks[j].public_key())
					);
				}
			}
			let file = dir.path().join(format!("node{i}.toml"));
			fs::write(&file, text).unwrap();
			let output = Command::new(env!("CARGO_BIN_EXE_rinpqc-node"))
				.arg("init")
				.arg(file)
				.output()
				.unwrap();
			assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
		}
		Self { dir, genesis, tx, first, submitter }
	}
	fn start(&self, excluded: Option<usize>, payments: bool) -> Running {
		let mut children = Vec::new();
		for i in 0..4 {
			if Some(i) == excluded {
				continue;
			}
			let child = self.start_node(i, payments);
			children.push((i, child));
		}
		Running { children, dir: self.dir.path().to_path_buf() }
	}
	fn start_node(&self, i: usize, payments: bool) -> Child {
		let mut cmd = Command::new(env!("CARGO_BIN_EXE_rinpqc-node"));
		cmd.arg("start").arg(self.dir.path().join(format!("node{i}.toml")));
		// Only the fourth scheduled proposer receives the local input. Inclusion by
		// height two therefore requires propagation to another proposer.
		if payments && i == self.submitter {
			cmd.arg(self.dir.path().join("payments.bin"));
		}
		cmd.stdout(Stdio::from(
			fs::File::create(self.dir.path().join(format!("out{i}.log"))).unwrap(),
		))
		.stderr(Stdio::from(fs::File::create(self.dir.path().join(format!("err{i}.log"))).unwrap()))
		.spawn()
		.unwrap()
	}
	fn states(&self, excluded: Option<usize>) -> Vec<rinpqc_node::application::execution::Ledger> {
		(0..4)
			.filter(|i| Some(*i) != excluded)
			.map(|i| {
				Store::open(
					&self.dir.path().join(format!("node{i}/application.redb")),
					&self.genesis,
				)
				.unwrap()
				.ledger()
				.unwrap()
				.clone()
			})
			.collect()
	}
}
impl Running {
	fn wait(&mut self, height: u64) {
		// This is a bounded integration-test deadline, not a consensus latency target.
		let until = Instant::now() + Duration::from_secs(120);
		loop {
			let mut ready = true;
			for (i, child) in &mut self.children {
				let errors = fs::read_to_string(self.dir.join(format!("err{i}.log"))).unwrap();
				assert!(child.try_wait().unwrap().is_none(), "node {i} exited: {errors}");
				let logs = fs::read_to_string(self.dir.join(format!("out{i}.log"))).unwrap();
				let committed = logs.contains(&format!("COMMITTED height={height} "));
				ready &= committed;
				assert!(
					committed || Instant::now() < until,
					"node {i} failed to commit {height}: {logs}\n{errors}"
				);
			}
			if ready {
				return;
			}
			thread::sleep(Duration::from_millis(30));
		}
	}
}
#[test]
fn four_validators_commit_payments_restart_and_replace_missing_proposer() {
	let network = Network::new();
	let mut nodes = network.start(None, true);
	nodes.wait(2);
	drop(nodes);
	let states = network.states(None);
	assert!(states.windows(2).all(|p| p[0] == p[1]));
	assert_eq!(states[0].account(&network.tx.transfer.sender).balance, 70);
	assert_eq!(states[0].account(&network.tx.transfer.sender).next_nonce, 1);
	assert_eq!(states[0].account(&network.tx.transfer.recipient).balance, 30);
	let mut nodes = network.start(None, true);
	nodes.wait(3);
	drop(nodes);
	let states = network.states(None);
	assert!(states.windows(2).all(|p| p[0] == p[1]));
	assert_eq!(states[0].account(&network.tx.transfer.sender).balance, 70);
	assert_eq!(states[0].account(&network.tx.transfer.sender).next_nonce, 1);
	fs::remove_file(network.dir.path().join("node0/consensus.wal")).unwrap();
	let failure = Command::new(env!("CARGO_BIN_EXE_rinpqc-node"))
		.arg("start")
		.arg(network.dir.path().join("node0.toml"))
		.output()
		.unwrap();
	assert!(!failure.status.success());
	assert!(String::from_utf8_lossy(&failure.stderr).contains("missing consensus WAL"));
	let network = Network::new();
	let excluded = Some(network.first);
	let mut nodes = network.start(excluded, false);
	nodes.wait(1);
	drop(nodes);
	let states = network.states(excluded);
	assert!(states.windows(2).all(|p| p[0] == p[1]));
	for i in (0..4).filter(|i| Some(*i) != excluded) {
		let logs = fs::read_to_string(network.dir.path().join(format!("out{i}.log"))).unwrap();
		assert!(logs.contains("height=1 round=1 "), "{logs}");
	}
}

#[test]
fn two_validators_cannot_finalize_and_can_restart_their_active_wal() {
	let network = Network::new();
	let excluded = Some(network.first);
	let mut nodes = network.start(excluded, false);
	let (removed, mut child) = nodes.children.pop().unwrap();
	child.kill().unwrap();
	child.wait().unwrap();
	let active: Vec<_> = nodes.children.iter().map(|(i, _)| *i).collect();
	thread::sleep(Duration::from_secs(7));
	for (i, child) in &mut nodes.children {
		assert!(child.try_wait().unwrap().is_none());
		let logs = fs::read_to_string(nodes.dir.join(format!("out{i}.log"))).unwrap();
		assert!(!logs.contains("COMMITTED"), "two votes finalized: {logs}");
	}
	drop(nodes);
	for i in active {
		// Only one recovering process is started at a time, so quorum remains impossible.
		let mut child = Command::new(env!("CARGO_BIN_EXE_rinpqc-node"))
			.arg("start")
			.arg(network.dir.path().join(format!("node{i}.toml")))
			.stdout(Stdio::null())
			.stderr(Stdio::from(
				fs::File::create(network.dir.path().join(format!("recovery{i}.log"))).unwrap(),
			))
			.spawn()
			.unwrap();
		thread::sleep(Duration::from_secs(2));
		let status = child.try_wait().unwrap();
		let _ = child.kill();
		let _ = child.wait();
		assert!(
			status.is_none(),
			"active WAL recovery failed: {}",
			fs::read_to_string(network.dir.path().join(format!("recovery{i}.log"))).unwrap()
		);
	}
	assert_ne!(Some(removed), excluded);
}

fn signed_history(network: &Network, index: usize) -> std::collections::BTreeMap<Vec<u8>, Vec<u8>> {
	use redb::{ReadableDatabase, TableDefinition};
	let database =
		redb::Database::open(network.dir.path().join(format!("node{index}/consensus.redb")))
			.unwrap();
	let read = database.begin_read().unwrap();
	let table = read.open_table(TableDefinition::<&[u8], &[u8]>::new("consensus_v1")).unwrap();
	table
		.range(b"s".as_slice()..b"t".as_slice())
		.unwrap()
		.map(|entry| {
			let (key, value) = entry.unwrap();
			(key.value().to_vec(), value.value().to_vec())
		})
		.collect()
}
fn certified_height(network: &Network, index: usize) -> u64 {
	use redb::{ReadableDatabase, TableDefinition};
	let database =
		redb::Database::open(network.dir.path().join(format!("node{index}/consensus.redb")))
			.unwrap();
	let read = database.begin_read().unwrap();
	let table = read.open_table(TableDefinition::<&[u8], &[u8]>::new("consensus_v1")).unwrap();
	table
		.range(b"c".as_slice()..b"d".as_slice())
		.unwrap()
		.map(|entry| {
			let (key, _) = entry.unwrap();
			u64::from_be_bytes(key.value()[1..].try_into().unwrap())
		})
		.max()
		.unwrap_or(0)
}
fn stop_one(nodes: &mut Running, index: usize) {
	let position = nodes.children.iter().position(|(i, _)| *i == index).unwrap();
	let (_, mut child) = nodes.children.remove(position);
	child.kill().unwrap();
	child.wait().unwrap();
}
fn assert_common_history(network: &Network, height: u64) {
	let stores: Vec<_> = (0..4)
		.map(|i| {
			Store::open(
				&network.dir.path().join(format!("node{i}/application.redb")),
				&network.genesis,
			)
			.unwrap()
		})
		.collect();
	for store in &stores {
		assert!(store.ledger().unwrap().height() >= height);
		assert_eq!(store.ledger().unwrap().account(&network.tx.transfer.sender).balance, 70);
		assert_eq!(store.ledger().unwrap().account(&network.tx.transfer.sender).next_nonce, 1);
		assert_eq!(store.ledger().unwrap().account(&network.tx.transfer.recipient).balance, 30);
		assert_eq!(
			store.receipt(&network.tx.transfer.id()).unwrap(),
			stores[0].receipt(&network.tx.transfer.id()).unwrap()
		);
		for h in 1..=height {
			assert_eq!(store.block(h).unwrap(), stores[0].block(h).unwrap());
		}
	}
}
#[test]
fn fresh_and_returning_validator_resume_verified_history_while_peers_keep_producing() {
	let network = Network::new();
	let late = network.first;
	let mut peers = network.start(Some(late), true);
	peers.wait(6);
	// This identity has never run before; its private directory still contains only genesis.
	let mut joining = Running {
		children: vec![(late, network.start_node(late, false))],
		dir: network.dir.path().to_path_buf(),
	};
	joining.wait(1);
	drop(joining); // Interrupt catch-up, retaining all durable files.
	let checkpoint = {
		let store = Store::open(
			&network.dir.path().join(format!("node{late}/application.redb")),
			&network.genesis,
		)
		.unwrap();
		store.ledger().unwrap().height()
	};
	assert!((1..6).contains(&checkpoint), "did not interrupt an incomplete download: {checkpoint}");
	// A crash can occur after the certificate is durable but before application commit.
	let certified = certified_height(&network, late);
	assert!((checkpoint..=checkpoint + 1).contains(&certified));
	let prior_signatures = signed_history(&network, late);
	peers.wait(8);
	peers.children.push((late, network.start_node(late, false)));
	peers.wait(10);
	let logs = fs::read_to_string(network.dir.path().join(format!("out{late}.log"))).unwrap();
	assert!(logs.contains(&format!("SIGNING_READY next_height={}", certified + 1)), "{logs}");
	assert!(logs.contains("SYNC_VERIFIED"), "{logs}");
	// Take a participating validator offline once more while the other three continue.
	stop_one(&mut peers, late);
	let before = signed_history(&network, late);
	for (slot, value) in prior_signatures {
		assert_eq!(before.get(&slot), Some(&value));
	}
	peers.wait(12);
	peers.children.push((late, network.start_node(late, false)));
	peers.wait(15);
	// Require its vote for further progress, rather than mistaking passive catch-up for readiness.
	let other = (0..4).find(|i| *i != late).unwrap();
	stop_one(&mut peers, other);
	peers.wait(19);
	drop(peers);
	assert_common_history(&network, 15);
	let after = signed_history(&network, late);
	for (slot, value) in before {
		assert_eq!(after.get(&slot), Some(&value));
	}
	assert!(
		after.keys().any(|key| u64::from_be_bytes(key[1..9].try_into().unwrap()) >= 14),
		"validator did not resume signing after catch-up: {}",
		fs::read_to_string(network.dir.path().join(format!("out{late}.log"))).unwrap()
	);
}
