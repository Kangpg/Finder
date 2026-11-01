//! Own the distinction between installed and portable application data.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

pub fn data_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("Locate Finder executable")?;
    let directory = exe
        .parent()
        .context("Finder executable has no parent directory")?;
    resolve(directory, std::env::var_os("LOCALAPPDATA").as_deref())
}
fn resolve(directory: &Path, local_app_data: Option<&OsStr>) -> Result<PathBuf> {
    match std::fs::read_to_string(directory.join(crate::product::INSTALL_MARKER)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(directory.join("data")),
        Err(error) => Err(error).context("Read Finder installation identity"),
        Ok(marker) => {
            ensure!(
                marker.trim_end_matches(['\r', '\n']) == crate::product::ID,
                "Unknown Finder installation identity; data location unchanged"
            );
            let local = PathBuf::from(
                local_app_data.context("LOCALAPPDATA is unavailable for installed Finder")?,
            );
            ensure!(local.is_absolute(), "LOCALAPPDATA must be an absolute path");
            Ok(local
                .join(crate::product::PUBLISHER)
                .join(crate::product::NAME)
                .join("data"))
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installation_identity_selects_stable_user_data_and_preserves_portable_data() {
        let portable = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve(portable.path(), None).unwrap(),
            portable.path().join("data")
        );
        let installed = tempfile::tempdir().unwrap();
        let marker = installed.path().join(crate::product::INSTALL_MARKER);
        std::fs::write(&marker, format!("{}\r\n", crate::product::ID)).unwrap();
        assert_eq!(
            resolve(installed.path(), Some(profile.path().as_os_str())).unwrap(),
            profile.path().join("Kangpg/Finder/data")
        );
        assert!(resolve(installed.path(), None).is_err());
        assert!(resolve(installed.path(), Some(OsStr::new("relative"))).is_err());
        std::fs::write(&marker, "foreign-product").unwrap();
        assert!(resolve(installed.path(), Some(profile.path().as_os_str())).is_err());
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "foreign-product");
    }
}
