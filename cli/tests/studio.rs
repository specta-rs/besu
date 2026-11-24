//! Process-level regressions for Studio's terminal output.

use std::process::Command;

#[test]
fn a_connection_error_does_not_print_the_database_password() {
    let output = Command::new(env!("CARGO_BIN_EXE_besu"))
        .args([
            "studio",
            "--database",
            "postgres://audit_user:AUDIT_DUMMY_PASSWORD_7f3a@127.0.0.1:not-a-port/audit_db",
        ])
        .output()
        .expect("run the real CLI");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "{:?}", output.stdout);
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 stderr");
    assert!(stderr.contains("Error: could not connect to"), "{stderr}");
    assert!(
        stderr.contains("postgres://audit_user@127.0.0.1:not-a-port/audit_db"),
        "{stderr}"
    );
    assert!(!stderr.contains("AUDIT_DUMMY_PASSWORD_7f3a"), "{stderr}");
}
