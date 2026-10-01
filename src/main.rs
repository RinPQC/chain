use std::process::ExitCode;

fn main() -> ExitCode {
	let args: Vec<_> = std::env::args_os().skip(1).collect();
	match args.as_slice() {
		[arg] if arg == "--help" || arg == "-h" => {
			println!("rinpqc-node — M1 workspace scaffold\n\nUsage: rinpqc-node --help | --version\n\nNode startup is not implemented yet.");
			ExitCode::SUCCESS
		},
		[arg] if arg == "--version" || arg == "-V" => {
			println!("rinpqc-node {}", env!("CARGO_PKG_VERSION"));
			ExitCode::SUCCESS
		},
		_ => {
			eprintln!("Node startup is not implemented. Use --help or --version.");
			ExitCode::FAILURE
		},
	}
}
