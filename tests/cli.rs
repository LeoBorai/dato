//! End-to-end tests of the `dato` binary.

use assert_cmd::Command;
use predicates::prelude::*;

fn dato() -> Command {
    Command::cargo_bin("dato").unwrap()
}

#[test]
fn one_shot_select_prints_table() {
    dato()
        .args([":memory:", "-f", "ascii", "-e", "select 1 as n;"])
        .assert()
        .success()
        .stdout("+---+\n| n |\n+---+\n| 1 |\n+---+\n");
}

#[test]
fn explicit_csv_format() {
    dato()
        .args([":memory:", "-f", "csv", "-e", "select 1 as a, 'x' as b;"])
        .assert()
        .success()
        .stdout("a,b\n1,x\n");
}

#[test]
fn piped_stdout_defaults_to_tsv() {
    dato()
        .args([":memory:", "-e", "select 1 as a, 2 as b;"])
        .assert()
        .success()
        .stdout("a\tb\n1\t2\n");
}

#[test]
fn no_target_uses_memory_database() {
    dato()
        .args(["-e", "select 42 as answer;"])
        .assert()
        .success()
        .stdout("answer\n42\n");
}

#[test]
fn script_on_stdin() {
    dato()
        .arg(":memory:")
        .write_stdin("create table t(a);\ninsert into t values (1);\nselect a from t;\n")
        .assert()
        .success()
        .stdout("a\n1\n");
}

#[test]
fn creates_and_reopens_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("new.db");
    dato()
        .arg(&db)
        .args(["-e", "create table t(a); insert into t values (5);"])
        .assert()
        .success();
    assert!(db.exists());
    dato()
        .arg(&db)
        .args(["-e", "select a from t;"])
        .assert()
        .success()
        .stdout("a\n5\n");
}

#[test]
fn failing_statement_exits_1_with_message_on_stderr() {
    dato()
        .args([":memory:", "-e", "select * from missing;"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no such table: missing"));
}

#[test]
fn statements_after_error_do_not_run() {
    dato()
        .args([":memory:", "-e", "select * from missing; select 1;"])
        .assert()
        .code(1)
        .stdout("");
}

#[test]
fn invalid_format_lists_valid_formats() {
    dato()
        .args(["-f", "bogus"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("valid formats"));
}

#[test]
fn postgres_url_is_unavailable() {
    dato()
        .arg("postgres://localhost/db")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Postgres backend is not available",
        ));
}

#[test]
fn mysql_url_is_unavailable() {
    dato()
        .arg("mysql://localhost/db")
        .assert()
        .failure()
        .stderr(predicate::str::contains("MySQL backend is not available"));
}

#[test]
fn version_flag() {
    dato()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with(format!(
            "dato {}",
            env!("CARGO_PKG_VERSION")
        )));
}

#[test]
fn help_flag() {
    dato()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--execute"));
}

#[test]
fn special_commands_run_in_scripts() {
    dato()
        .arg(":memory:")
        .write_stdin("create table users(id);\n.tables\n\\T csv\nselect 1 as a;\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("users").and(predicate::str::contains("a\n1\n")));
}
