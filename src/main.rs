use rand_core::{OsRng, RngCore};
use rinpqc_node::{
	crypto::{Error, KeyRole, Result, SecretKey},
	genesis::{parse_id, Genesis, MAX_GENESIS_JSON},
	infrastructure::{read_bounded, save_key, write_new, NodeConfig},
};
use std::{env, path::Path, process::ExitCode};

fn run(args: &[String]) -> eyre::Result<()> {
	match args {
		[arg] if arg == "--help" || arg == "-h" => {
			println!("rinpqc-node — M1 development node\n\nCommands:\n  keygen <transaction|validator|network> <new-key-file>\n  genesis-create <new-json-file> <validator1> <validator2> <validator3> <validator4> <funded-account>\n  genesis-check <json-file>\n  config-check <toml-file>\n  init <toml-file>\n  start <toml-file> [signed-payment-batch]\n  --version\n\nKeys and account IDs are lowercase hex public keys.");
		},
		[arg] if arg == "--version" || arg == "-V" => {
			println!("rinpqc-node {}", env!("CARGO_PKG_VERSION"))
		},
		[cmd, role, path] if cmd == "keygen" => {
			let role = match role.as_str() {
				"transaction" => KeyRole::Transaction,
				"validator" => KeyRole::Validator,
				"network" => KeyRole::Network,
				_ => return Err(Error::Role.into()),
			};
			let key = SecretKey::generate(role)?;
			save_key(Path::new(path), &key)?;
			println!("{}", hex::encode(key.public_key()));
		},
		[cmd, path, a, b, c, d, account] if cmd == "genesis-create" => {
			let mut validators = [parse_id(a)?, parse_id(b)?, parse_id(c)?, parse_id(d)?];
			validators.sort();
			let mut network_nonce = [0; 32];
			OsRng
				.try_fill_bytes(&mut network_nonce)
				.map_err(|_| Error::Config("OS randomness unavailable"))?;
			let genesis = Genesis {
				network_nonce,
				max_block_bytes: 1_048_576,
				max_transactions: 4096,
				target_interval_ms: 5000,
				validators,
				accounts: vec![(parse_id(account)?, 4_000_000)],
			};
			write_new(Path::new(path), &genesis.to_json()?)?;
			print_genesis(&genesis)?;
		},
		[cmd, path] if cmd == "genesis-check" => {
			let genesis = Genesis::from_json(&read_bounded(Path::new(path), MAX_GENESIS_JSON)?)?;
			print_genesis(&genesis)?;
		},
		[cmd, path] if cmd == "config-check" || cmd == "init" => {
			let config = NodeConfig::load(Path::new(path))?;
			if cmd == "init" {
				rinpqc_node::consensus::initialize(&config)?;
			}
			print_genesis(&config.genesis)?;
			println!("validator={}", hex::encode(config.validator.public_key()));
			println!("network={}", hex::encode(config.network.public_key()));
		},
		[cmd, path, rest @ ..] if cmd == "start" && rest.len() <= 1 => {
			let config = NodeConfig::load(Path::new(path))?;
			let payments = rest
				.first()
				.map(|p| rinpqc_node::consensus::load_payments(Path::new(p)))
				.transpose()?
				.unwrap_or_default();
			tracing_subscriber::fmt()
				.with_env_filter(
					tracing_subscriber::EnvFilter::try_from_default_env()
						.unwrap_or_else(|_| "warn".into()),
				)
				.with_writer(std::io::stderr)
				.try_init()
				.ok();
			tokio::runtime::Builder::new_multi_thread()
				.worker_threads(2)
				.enable_all()
				.build()?
				.block_on(rinpqc_node::consensus::run(config, payments))?;
		},
		_ => {
			return Err(Error::Config("Invalid command. Use --help for available commands.").into())
		},
	}
	Ok(())
}
fn print_genesis(genesis: &Genesis) -> Result<()> {
	println!("chain_id={}", hex::encode(genesis.chain_id()?));
	println!("state_root={}", hex::encode(genesis.state_root()?));
	Ok(())
}
fn main() -> ExitCode {
	let args = env::args_os()
		.skip(1)
		.map(|s| s.into_string().map_err(|_| Error::Encoding))
		.collect::<Result<Vec<_>>>();
	match args.map_err(eyre::Report::from).and_then(|args| run(&args)) {
		Ok(()) => ExitCode::SUCCESS,
		Err(error) => {
			eprintln!("{error}");
			ExitCode::FAILURE
		},
	}
}
