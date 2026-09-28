use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

pub fn recordings_dir() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("Could not locate your home directory")?
        .join("Videos/Recordings"))
}

/// Reserve a unique file without ever truncating an earlier recording.
pub struct Output {
    pub partial: PathBuf,
    pub destination: PathBuf,
}

impl Output {
    pub fn reserve(directory: &Path) -> Result<Self> {
        Self::reserve_named(
            directory,
            &chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string(),
        )
    }

    fn reserve_named(directory: &Path, timestamp: &str) -> Result<Self> {
        fs::create_dir_all(directory)
            .with_context(|| format!("Cannot create {}", directory.display()))?;
        for suffix in 0..10_000 {
            let name = if suffix == 0 {
                format!("Recording_{timestamp}.mkv")
            } else {
                format!("Recording_{timestamp}_{suffix}.mkv")
            };
            let destination = directory.join(&name);
            if destination.exists() {
                continue;
            }
            let partial = directory.join(format!("{name}.part"));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)
            {
                Ok(_) => {
                    return Ok(Self {
                        partial,
                        destination,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("Cannot write to {}", directory.display()));
                }
            }
        }
        bail!("Too many recordings with the same timestamp")
    }

    pub fn finish(self) -> Result<PathBuf> {
        let file = File::open(&self.partial)?;
        if file.metadata()?.len() == 0 {
            bail!("The recording is empty");
        }
        file.sync_all()
            .context("Could not flush the recording to disk")?;
        // Both names are in the same directory. A hard link publishes atomically
        // and, unlike rename on Unix, refuses to overwrite an existing file.
        fs::hard_link(&self.partial, &self.destination).with_context(|| {
            format!(
                "Could not save {}; the recording remains at {}",
                self.destination.display(),
                self.partial.display()
            )
        })?;
        fs::remove_file(&self.partial)?;
        Ok(self.destination.clone())
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        if fs::metadata(&self.partial).is_ok_and(|m| m.len() == 0) {
            let _ = fs::remove_file(&self.partial);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simultaneous_recordings_do_not_collide_or_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let first = Output::reserve_named(dir.path(), "same-time").unwrap();
        let second = Output::reserve_named(dir.path(), "same-time").unwrap();
        assert_ne!(first.partial, second.partial);
        fs::write(&first.partial, b"first").unwrap();
        let saved = first.finish().unwrap();
        fs::write(&second.partial, b"second").unwrap();
        second.finish().unwrap();
        assert_eq!(fs::read(saved).unwrap(), b"first");
    }

    #[test]
    fn failed_publish_preserves_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let out = Output::reserve(dir.path()).unwrap();
        fs::write(&out.partial, b"new").unwrap();
        fs::write(&out.destination, b"existing").unwrap();
        let partial = out.partial.clone();
        let destination = out.destination.clone();
        assert!(out.finish().is_err());
        assert_eq!(fs::read(destination).unwrap(), b"existing");
        assert_eq!(fs::read(partial).unwrap(), b"new");
    }

    #[test]
    fn empty_and_invalid_outputs_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let out = Output::reserve(dir.path()).unwrap();
        let partial = out.partial.clone();
        assert!(out.finish().is_err());
        assert!(!partial.exists());
        let file = dir.path().join("not-a-directory");
        fs::write(&file, b"occupied").unwrap();
        assert!(Output::reserve(&file).is_err());
    }

    #[test]
    fn unwritable_directory_is_reported_without_creating_a_recording() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
        let result = Output::reserve(dir.path());
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err());
        assert_eq!(dir.path().read_dir().unwrap().count(), 0);
    }
}
