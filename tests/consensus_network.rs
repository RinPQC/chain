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
		Self { dir, genesis, tx, first }
	}
	fn start(&self, excluded: Option<usize>, payments: bool) -> Running {
		let mut children = Vec::new();
		for i in 0..4 {
			if Some(i) == excluded {
				continue;
			}
			let mut cmd = Command::new(env!("CARGO_BIN_EXE_rinpqc-node"));
			cmd.arg("start").arg(self.dir.path().join(format!("node{i}.toml")));
			if payments {
				cmd.arg(self.dir.path().join("payments.bin"));
			}
			let child = cmd
				.stdout(Stdio::from(
					fs::File::create(self.dir.path().join(format!("out{i}.log"))).unwrap(),
				))
				.stderr(Stdio::from(
					fs::File::create(self.dir.path().join(format!("err{i}.log"))).unwrap(),
				))
				.spawn()
				.unwrap();
			children.push((i, child));
		}
		Running { children, dir: self.dir.path().to_path_buf() }
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
		let until = Instant::now() + Duration::from_secs(55);
		loop {
			let mut ready = true;
			for (i, child) in &mut self.children {
				let errors = fs::read_to_string(self.dir.join(format!("err{i}.log"))).unwrap();
				assert!(child.try_wait().unwrap().is_none(), "node {i} exited: {errors}");
				let logs = fs::read_to_string(self.dir.join(format!("out{i}.log"))).unwrap();
				ready &= logs.contains(&format!("COMMITTED height={height} "));
				assert!(
					Instant::now() < until,
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
