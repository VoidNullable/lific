use std::process::Command;

fn completion_with_worker_threads(value: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lific"))
        .env("TOKIO_WORKER_THREADS", value)
        .args(["completion", "bash"])
        .output()
        .expect("run lific completion subprocess")
}

#[test]
fn tokio_worker_thread_override_and_validation_are_preserved() {
    for value in ["1", "2"] {
        let output = completion_with_worker_threads(value);
        assert!(
            output.status.success(),
            "supported TOKIO_WORKER_THREADS={value} should succeed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    for value in ["0", "not-a-number"] {
        let output = completion_with_worker_threads(value);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "invalid override {value} must fail"
        );
        assert!(
            stderr.contains("TOKIO_WORKER_THREADS"),
            "Tokio should report the invalid override, got: {stderr}"
        );
    }
}
