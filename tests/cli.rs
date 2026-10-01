use std::process::Command;

#[test]
fn scaffold_never_reports_a_running_node() {
	// Until startup exists, a launch attempt must fail visibly rather than
	// fooling an operator or supervisor into treating the scaffold as a node.
	for args in [vec![], vec!["start"], vec!["--version", "start"]] {
		let result = Command::new(env!("CARGO_BIN_EXE_rinpqc-node"))
			.args(args)
			.output()
			.expect("launch scaffold");
		assert!(!result.status.success());
		assert!(result.stdout.is_empty());
		assert!(String::from_utf8_lossy(&result.stderr).contains("not implemented"));
	}
}
