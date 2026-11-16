use serde_json::Value;
use std::{fs, process::Command};

fn run(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_romstore-server"))
        .args(["index", "--library"])
        .arg(root.join("library"))
        .arg("--database")
        .arg(root.join("catalog.sqlite3"))
        .args(args)
        .env_remove("RUST_LOG")
        .output()
        .unwrap()
}

#[test]
fn cli_progress_stderr_json_stdout_cache_and_log_levels() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("library/nes");
    fs::create_dir_all(&system).unwrap();
    let mut xml = String::from("<gameList>");
    for index in 0..205 {
        let name = format!("demo{index}.nes");
        fs::write(system.join(&name), b"synthetic rom").unwrap();
        xml.push_str(&format!("<game><path>{name}</path></game>"));
    }
    xml.push_str("</gameList>");
    fs::write(system.join("gamelist.xml"), xml).unwrap();
    let output = run(temp.path(), &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["games"],
        205
    );
    let logs = String::from_utf8(output.stderr).unwrap();
    assert!(logs.contains("Indexation terminée"));
    assert!(logs.contains("100/205 entrées traitées"));
    assert!(logs.contains("200/205 entrées traitées"));
    assert!(logs.contains("0 empreintes réutilisées, 205 calculées"));
    assert!(logs.lines().count() < 25);
    let output = run(temp.path(), &["--log-level", "debug"]);
    assert!(output.status.success());
    let logs = String::from_utf8(output.stderr).unwrap();
    assert!(logs.contains("Empreinte réutilisée"));
    assert!(logs.contains("205 empreintes réutilisées, 0 calculées"));
    let output = run(temp.path(), &["--log-level", "error"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["systems"],
        1
    );
    assert!(!temp.path().join("system-icons").exists());
}

#[test]
fn help_and_invalid_configuration_keep_existing_cli_options() {
    let binary = env!("CARGO_BIN_EXE_romstore-server");
    let help = Command::new(binary).arg("--help").output().unwrap();
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    for option in [
        "--library",
        "--database",
        "--system-icon",
        "--host",
        "--port",
        "--log-level",
    ] {
        assert!(text.contains(option));
    }
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("library")).unwrap();
    let file = temp.path().join("not-a-directory");
    fs::write(&file, b"file").unwrap();
    let output = run(temp.path(), &["--system-icon", file.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("--system-icon"));
    let output = run(
        temp.path(),
        &[
            "--database",
            temp.path()
                .join("library/catalog.sqlite3")
                .to_str()
                .unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(2));
}

#[tokio::test]
async fn serve_opens_http_even_when_startup_indexation_cannot_run() {
    use fs2::FileExt;
    use romstore_server::{catalog::Catalog, config::Config};
    use std::{process::Stdio, time::Duration};

    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("library/nes");
    fs::create_dir_all(&system).unwrap();
    fs::write(system.join("demo.nes"), b"rom").unwrap();
    fs::write(
        system.join("gamelist.xml"),
        "<gameList><game><path>demo.nes</path></game></gameList>",
    )
    .unwrap();
    let database = temp.path().join("catalog.sqlite3");
    let catalog =
        Catalog::new(Config::new(system.parent().unwrap(), &database, None).unwrap()).unwrap();
    catalog.reindex().unwrap();
    // An external indexer owns the lock: startup must not wait or exit on failure.
    let lock = fs::File::create(database.with_extension("index.lock")).unwrap();
    lock.lock_exclusive().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut server = tokio::process::Command::new(env!("CARGO_BIN_EXE_romstore-server"))
        .arg("serve")
        .arg("--library")
        .arg(system.parent().unwrap())
        .arg("--database")
        .arg(&database)
        .arg("--port")
        .arg(address.port().to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let base = format!("http://{address}");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            assert!(
                server.try_wait().unwrap().is_none(),
                "Server exited before opening HTTP"
            );
            if let Ok(response) = http
                .get(format!("{base}/api/v1/catalog/reload"))
                .send()
                .await
            {
                let state: Value = response.json().await.unwrap();
                if state["status"] == "failed" {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let summary: Value = http
        .get(format!("{base}/api/v1/catalog/summary"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        summary[0]["games"], 1,
        "The persisted catalogue remains available"
    );
    drop(lock);
    let response = http
        .post(format!("{base}/api/v1/catalog/reload"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    server.kill().await.unwrap();
}
