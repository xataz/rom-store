//! Optional local frontend refresh, after installation writes and SDL shutdown.
//! Frontend commands come only from the user's local configuration, never the API.
use crate::config::{Config, Frontend};
use anyhow::{bail, Context, Result};
use std::{
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const REFRESH_TIMEOUT: Duration = Duration::from_secs(5);

pub fn refresh(config: &Config) -> Result<()> {
    if !config.auto_refresh {
        return Ok(());
    }
    if let Some((program, args)) = config.refresh_command.split_first() {
        run_command(program, args, REFRESH_TIMEOUT)?;
    } else if config.frontend == Frontend::Batocera {
        refresh_batocera("http://127.0.0.1:1234/reloadgames", REFRESH_TIMEOUT)?;
    } else {
        bail!(
            "Aucun refresh natif pour {:?} : configurer refresh_command",
            config.frontend
        );
    }
    eprintln!(
        "Refresh de la liste des jeux demandé au frontend {:?}",
        config.frontend
    );
    Ok(())
}

/// A failed refresh must not turn a successfully completed installation into an error.
pub fn refresh_on_exit(config: &Config) {
    if let Err(error) = refresh(config) {
        eprintln!("Refresh automatique du frontend impossible : {error:#}. Actualiser la liste des jeux manuellement.");
    }
}

fn refresh_batocera(url: &str, timeout: Duration) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let response = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()?
            .get(url)
            .send()
            .await
            .context("API locale EmulationStation inaccessible")?;
        if !response.status().is_success() {
            bail!(
                "Le frontend a refusé le refresh (HTTP {})",
                response.status()
            );
        }
        Ok(())
    })
}

fn run_command(program: &str, args: &[String], timeout: Duration) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| format!("Impossible de lancer la commande de refresh {program:?}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => bail!("La commande de refresh a échoué ({status})"),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                match result {
                    Err(error) => return Err(error.into()),
                    _ => bail!("Délai de la commande de refresh dépassé"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };

    #[test]
    fn disabled_refresh_never_executes_a_configured_command() {
        let config = Config {
            auto_refresh: false,
            refresh_command: vec!["/does/not/exist".into()],
            ..Config::default()
        };
        refresh(&config).unwrap();
    }

    #[test]
    fn batocera_requests_only_local_reloadgames_and_checks_status() {
        for status in [200, 302, 500] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/reloadgames", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(line.trim(), "GET /reloadgames HTTP/1.1");
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                }
                write!(stream, "HTTP/1.1 {status} Test\r\nLocation: http://127.0.0.1:1/unwanted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            });
            assert_eq!(
                refresh_batocera(&url, Duration::from_secs(2)).is_ok(),
                status == 200
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn local_command_is_opt_in_preserves_arguments_and_does_not_invoke_a_shell() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("output");
        let literal = format!(
            "$(touch {}) ; not shell syntax",
            dir.path().join("unwanted").display()
        );
        let config = Config {
            frontend: Frontend::Custom,
            auto_refresh: true,
            // The test explicitly chooses a shell as its executable. Production
            // code never adds one or interpolates command arguments.
            refresh_command: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf '%s' \"$1\" > \"$2\"".into(),
                "refresh-test".into(),
                literal.clone(),
                output.display().to_string(),
            ],
            ..Config::default()
        };
        refresh(&config).unwrap();
        assert_eq!(std::fs::read_to_string(output).unwrap(), literal);
        assert!(!dir.path().join("unwanted").exists());
    }

    #[test]
    fn commands_report_failure_and_are_bounded_in_time() {
        assert!(run_command("/bin/false", &[], Duration::from_secs(1)).is_err());
        assert!(run_command("/does/not/exist", &[], Duration::from_secs(1)).is_err());
        let started = Instant::now();
        assert!(run_command("/bin/sleep", &["5".into()], Duration::from_millis(50)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
