#[derive(Debug, Clone, PartialEq, Eq, Hash, serde_derive::Deserialize)]
#[allow(non_camel_case_types)]
pub enum Arch {
    #[serde(rename = "i686")]
    I686,
    #[serde(rename = "x86_64")]
    X86_64,
    #[serde(rename = "arm")]
    ARM,
    #[serde(rename = "armv6h")]
    ARMV6H,
    #[serde(rename = "armv7h")]
    ARMV7H,
    #[serde(rename = "aarch64")]
    AARCH64,
}

impl std::fmt::Display for Arch {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        match *self {
            Arch::I686 => "i686".fmt(f),
            Arch::X86_64 => "x86_64".fmt(f),
            Arch::ARM => "arm".fmt(f),
            Arch::ARMV6H => "armv6h".fmt(f),
            Arch::ARMV7H => "armv7h".fmt(f),
            Arch::AARCH64 => "aarch64".fmt(f),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChrootHelper<'a> {
    chroot_dir: &'a str,
    arch: Arch,
}

impl<'a> ChrootHelper<'a> {
    pub fn new(chroot_dir: &'a str, arch: Arch) -> Self {
        ChrootHelper { chroot_dir, arch }
    }

    pub fn makechrootpkg<P, Q, R, S>(
        &self,
        package_dir: P,
        srcdest: Q,
        pkgdest: R,
        logdest: S,
    ) -> Result<(), failure::Error>
    where
        P: AsRef<std::path::Path>,
        Q: AsRef<std::path::Path>,
        R: AsRef<std::path::Path>,
        S: AsRef<std::path::Path>,
    {
        let current_dir_buf = std::env::current_dir()?;
        let current_dir = current_dir_buf.as_path();
        let mut srcdest_arg = std::ffi::OsString::from("SRCDEST=");
        srcdest_arg.push(current_dir.join(srcdest));
        let mut pkgdest_arg = std::ffi::OsString::from("PKGDEST=");
        pkgdest_arg.push(current_dir.join(pkgdest));
        let mut logdest_arg = std::ffi::OsString::from("LOGDEST=");
        logdest_arg.push(current_dir.join(logdest));

        let mut cmd = std::process::Command::new("sudo");
        cmd.current_dir(package_dir)
            .arg("env")
            .arg(srcdest_arg)
            .arg(pkgdest_arg)
            .arg(logdest_arg)
            .arg("makechrootpkg")
            .arg("-cur")
            .arg(current_dir.join(self.chroot_dir));
        log::info!("{:?}", cmd);
        let status = cmd.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(failure::format_err!("makechrootpkg failed"))
        }
    }
}

#[derive(Debug, Clone)]
pub struct Builder<'a> {
    signer: Option<&'a super::signer::Signer<'a>>,
    srcdest: &'a str,
    logdest: &'a str,
}

impl<'a> Builder<'a> {
    pub fn new(
        signer: Option<&'a super::signer::Signer<'a>>,
        srcdest: &'a str,
        logdest: &'a str,
    ) -> Self {
        Builder {
            signer,
            srcdest,
            logdest,
        }
    }

    pub fn build_package<P, Q>(
        &self,
        package_dir: P,
        repo_dir: Q,
        chroot_helper: &ChrootHelper,
    ) -> Result<Vec<std::path::PathBuf>, failure::Error>
    where
        P: AsRef<std::path::Path>,
        Q: AsRef<std::path::Path>,
    {
        let package_dir = package_dir.as_ref();
        let tempdir = tempdir::TempDir::new("guzuta-pkgdest")?;
        let pkgdest = tempdir.path();
        chroot_helper.makechrootpkg(package_dir, self.srcdest, pkgdest, self.logdest)?;
        let mut paths = vec![];
        for entry in std::fs::read_dir(pkgdest)? {
            let entry = entry?;
            let symlink_package_path = package_dir.join(entry.file_name());
            if symlink_package_path.read_link().is_ok() {
                // Unlink symlink created by makechrootpkg
                log::info!("Unlink symlink {}", symlink_package_path.display());
                std::fs::remove_file(symlink_package_path)?;
            }
            let dest = repo_dir.as_ref().join(entry.file_name());
            log::info!("Copy {} to {}", entry.path().display(), dest.display());
            std::fs::copy(entry.path(), &dest)?;
            if let Some(signer) = self.signer {
                let mut sig_dest = dest.clone().into_os_string();
                sig_dest.push(".sig");
                signer.sign(&dest, sig_dest)?;
            }
            paths.push(dest);
        }
        Ok(paths)
    }
}
