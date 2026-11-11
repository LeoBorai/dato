//! End-to-end tests of the `dato` binary.

use assert_cmd::Command;
use predicates::prelude::*;

fn dato() -> Command {
    Command::cargo_bin("dato").unwrap()
}

#[test]
fn one_shot_select_prints_table() {
    dato()
        .args([":memory:", "-T", "ascii", "-e", "select 1 as n;"])
        .assert()
        .success()
        .stdout("+---+\n| n |\n+---+\n| 1 |\n+---+\n");
}

#[test]
fn explicit_csv_format() {
    dato()
        .args([":memory:", "-T", "csv", "-e", "select 1 as a, 'x' as b;"])
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
fn long_format_flag() {
    dato()
        .args([":memory:", "--format", "csv", "-e", "select 1 as a;"])
        .assert()
        .success()
        .stdout("a\n1\n");
}

#[test]
fn short_f_is_not_accepted() {
    dato()
        .args([":memory:", "-f", "csv", "-e", "select 1;"])
        .assert()
        .failure();
}

#[test]
fn one_shot_without_target_is_an_error() {
    dato()
        .args(["-e", "select 42 as answer;"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicate::str::contains("database target is required"));
}

#[test]
fn piped_input_without_target_is_an_error() {
    dato()
        .write_stdin("select 1;\n")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("database target is required"));
}

#[test]
fn sqlite_subcommand_matches_shorthand() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    dato()
        .arg("sqlite")
        .arg(&db)
        .args(["-e", "create table t(a); insert into t values (3);"])
        .assert()
        .success();
    dato()
        .arg(&db)
        .args(["-e", "select a from t;"])
        .assert()
        .success()
        .stdout("a\n3\n");
}

#[test]
fn connect_is_rejected_in_scripts() {
    dato()
        .arg(":memory:")
        .write_stdin("\\connect sqlite a.db\n")
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "\\connect is only available in the interactive shell",
        ));
}

#[test]
fn mysql_help_lists_login_arguments() {
    dato().args(["mysql", "--help"]).assert().success().stdout(
        predicate::str::contains("-h, --host")
            .and(predicate::str::contains("-P, --port"))
            .and(predicate::str::contains("-u, --user"))
            .and(predicate::str::contains("-p, --password"))
            .and(predicate::str::contains("-S, --socket"))
            .and(predicate::str::contains("-D, --database")),
    );
}

#[test]
fn psql_help_lists_only_psql_arguments() {
    dato().args(["psql", "--help"]).assert().success().stdout(
        predicate::str::contains("-h, --host")
            .and(predicate::str::contains("-p, --port"))
            .and(predicate::str::contains("-U, --username"))
            .and(predicate::str::contains("-d, --dbname"))
            .and(predicate::str::contains("-W, --password"))
            .and(predicate::str::contains("--sslmode"))
            .and(predicate::str::contains("--socket").not()),
    );
}

#[test]
fn psql_rejects_other_servers_flags_and_bad_sslmode() {
    dato().args(["psql", "-P", "5433"]).assert().failure();
    dato()
        .args(["psql", "-U", "x", "-Wsecret"])
        .assert()
        .failure();
    dato()
        .args(["psql", "--sslmode", "bogus"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "disable, prefer, require, verify-full",
        ));
}

#[test]
fn postgres_url_target_suggests_subcommands() {
    dato()
        .arg("postgres://app@localhost/shop")
        .assert()
        .failure()
        .stderr(predicate::str::contains("dato psql"));
}

#[test]
fn mysql_attached_password_is_rejected() {
    dato()
        .args(["mysql", "-u", "root", "-psecret"])
        .assert()
        .failure();
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
        .args(["-T", "bogus"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("valid formats"));
}

#[test]
fn url_targets_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    for url in ["postgres://localhost/db", "mysql://root@localhost/shop"] {
        dato()
            .current_dir(dir.path())
            .arg(url)
            .assert()
            .failure()
            .stderr(predicate::str::contains("dato mysql"));
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
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
