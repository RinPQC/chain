use std::process::Command;

#[test]
fn incomplete_launch_commands_fail_visibly() {
	// A malformed launch must not fool a supervisor into treating it as a running node.
	for args in [vec![], vec!["start"], vec!["--version", "start"]] {
		let result = Command::new(env!("CARGO_BIN_EXE_rinpqc-node"))
			.args(args)
			.output()
			.expect("launch scaffold");
		assert!(!result.status.success());
		assert!(result.stdout.is_empty());
		assert!(String::from_utf8_lossy(&result.stderr).contains("Invalid command"));
	}
}
