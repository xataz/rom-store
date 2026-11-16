use crate::paths::{regular_file, Signature};
use anyhow::{bail, Result};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::Path,
    time::{Duration, Instant},
};
use tracing::{debug, info};

pub const INTERVAL: Duration = Duration::from_secs(5);
pub const BATCH: usize = 100;

pub fn hash_file(
    path: &Path,
    signature: &Signature,
    system: &str,
    relative: &str,
) -> Result<String> {
    let started = Instant::now();
    let mut last_log = started;
    let large = signature.2 >= 256 * 1024 * 1024;
    if large {
        info!(
            "[{system}] Calcul SHA-256 : {relative:?} ({:.1} Mio)",
            signature.2 as f64 / 1048576.0
        );
    } else {
        debug!("[{system}] Calcul SHA-256 : {relative:?}");
    }
    let mut file = regular_file(path)?;
    if Signature::of(&file.metadata()?) != *signature {
        bail!("Fichier modifié avant lecture");
    }
    let mut digest = Sha256::new();
    let mut completed = 0u64;
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        completed += count as u64;
        if last_log.elapsed() >= INTERVAL {
            info!(
                "[{system}] SHA-256 {relative:?} : {:.1} / {:.1} Mio ({:.1} %) — {:.1} Mio/s",
                completed as f64 / 1048576.0,
                signature.2 as f64 / 1048576.0,
                (completed as f64 / signature.2.max(1) as f64 * 100.0).min(100.0),
                completed as f64 / 1048576.0 / started.elapsed().as_secs_f64().max(0.001)
            );
            last_log = Instant::now();
        }
    }
    if Signature::of(&file.metadata()?) != *signature
        || Signature::of(&std::fs::metadata(path)?) != *signature
        || completed != signature.2
    {
        bail!("Fichier modifié pendant l'indexation");
    }
    if large || started.elapsed() >= INTERVAL {
        info!(
            "[{system}] SHA-256 terminé : {relative:?} — {:.1} Mio en {:.1} s",
            completed as f64 / 1048576.0,
            started.elapsed().as_secs_f64()
        );
    }
    Ok(hex::encode(digest.finalize()))
}
