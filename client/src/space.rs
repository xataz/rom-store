//! Native free-space queries; no platform-specific shell commands are executed.
use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
};

pub const SAFETY_MARGIN: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct InsufficientSpace {
    pub path: PathBuf,
    pub required: u64,
    pub available: u64,
}

impl fmt::Display for InsufficientSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Espace disque insuffisant sur {} : {:.2} Mio nécessaires, {:.2} Mio disponibles (marge de 16 Mio incluse)",
            self.path.display(), self.required as f64 / 1048576.0, self.available as f64 / 1048576.0)
    }
}
impl std::error::Error for InsufficientSpace {}

pub(crate) struct Snapshot {
    pub volume: String,
    pub available: u64,
    pub allocation: u64,
}

#[cfg(unix)]
fn volume_id(path: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::metadata(path)?.dev().to_string())
}

#[cfg(windows)]
fn volume_id(path: &Path) -> Result<String> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::{
        GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut mount = vec![0_u16; 32768];
    // Both buffers are writable, sized in UTF-16 code units; the input is NUL-terminated.
    if unsafe { GetVolumePathNameW(path.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut volume = vec![0_u16; 32768];
    // Prefer a volume GUID so two local mount points for the same disk aggregate.
    // Network shares can lack a GUID; their native mount root is then the identity.
    let buffer = if unsafe {
        GetVolumeNameForVolumeMountPointW(mount.as_ptr(), volume.as_mut_ptr(), volume.len() as u32)
    } != 0
    {
        &volume
    } else {
        &mount
    };
    let end = buffer
        .iter()
        .position(|c| *c == 0)
        .context("Nom de volume Windows invalide")?;
    Ok(std::ffi::OsString::from_wide(&buffer[..end])
        .to_string_lossy()
        .to_lowercase())
}

pub(crate) fn snapshot(path: &Path) -> Result<Snapshot> {
    let stats = fs2::statvfs(path)
        .with_context(|| format!("Espace disponible inaccessible : {}", path.display()))?;
    Ok(Snapshot {
        volume: volume_id(path)?,
        available: stats.available_space(),
        allocation: stats.allocation_granularity().max(1),
    })
}

/// Sum requirements by filesystem, not by folder. Round each new file to its
/// allocation unit; use the least free space observed if another writer is active.
pub(crate) fn ensure(
    requirements: impl IntoIterator<Item = (PathBuf, u64)>,
    probe: &impl Fn(&Path) -> Result<Snapshot>,
) -> Result<()> {
    let mut volumes = BTreeMap::<String, (PathBuf, u64, u64)>::new();
    for (path, bytes) in requirements {
        let stats = probe(&path)?;
        let unit = stats.allocation.max(1);
        let allocated = bytes
            .div_ceil(unit)
            .checked_mul(unit)
            .context("Taille allouée trop grande")?;
        let (_, total, free) = volumes
            .entry(stats.volume)
            .or_insert((path, 0, stats.available));
        *total = total
            .checked_add(allocated)
            .context("Taille requise trop grande")?;
        *free = (*free).min(stats.available);
    }
    for (_, (path, bytes, available)) in volumes {
        let required = bytes
            .checked_add(SAFETY_MARGIN)
            .context("Taille requise trop grande")?;
        if required > available {
            return Err(InsufficientSpace {
                path,
                required,
                available,
            }
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combines_folders_on_same_volume_and_rounds_allocations() {
        let requirements = vec![(PathBuf::from("roms"), 5), (PathBuf::from("media"), 5)];
        let probe = |_: &Path| {
            Ok(Snapshot {
                volume: "same disk".into(),
                available: SAFETY_MARGIN + 15,
                allocation: 4,
            })
        };
        let error = ensure(requirements, &probe).unwrap_err();
        let space = error.downcast_ref::<InsufficientSpace>().unwrap();
        assert_eq!(space.required, SAFETY_MARGIN + 16);
        assert_eq!(space.available, SAFETY_MARGIN + 15);
    }

    #[test]
    fn separate_media_disk_does_not_need_room_for_the_rom() {
        let probe = |path: &Path| {
            Ok(Snapshot {
                volume: path.display().to_string(),
                available: if path == Path::new("roms") {
                    SAFETY_MARGIN + 1000
                } else {
                    SAFETY_MARGIN + 10
                },
                allocation: 1,
            })
        };
        ensure(vec![("roms".into(), 1000), ("media".into(), 10)], &probe).unwrap();
    }

    #[test]
    fn rejects_overflow_and_failed_queries_instead_of_skipping_the_check() {
        let probe = |_: &Path| {
            Ok(Snapshot {
                volume: "disk".into(),
                available: u64::MAX,
                allocation: 4096,
            })
        };
        assert!(ensure(vec![("roms".into(), u64::MAX)], &probe).is_err());
        assert!(ensure(vec![("roms".into(), 1)], &|_| anyhow::bail!("query failed")).is_err());
    }

    #[test]
    fn native_snapshot_works_for_a_real_destination() {
        let root = tempfile::tempdir().unwrap();
        let stats = snapshot(root.path()).unwrap();
        assert!(!stats.volume.is_empty());
        assert!(stats.allocation > 0);
    }
}
