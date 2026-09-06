use std::process::Command;

#[test]
fn help_names_the_api_key_variable_without_printing_its_value() {
    let secret = "lific-help-must-not-print-this-test-key";
    for args in [vec!["--help"], vec!["doctor", "--help"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_lific"))
            .env("LIFIC_API_KEY", secret)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stdout.contains("LIFIC_API_KEY"));
        assert!(!stdout.contains(secret));
        assert!(!stderr.contains(secret));
    }
}
