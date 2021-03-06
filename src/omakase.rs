#[derive(Debug, serde_derive::Deserialize)]
pub struct Config {
    pub name: String,
    pub package_key: Option<String>,
    pub repo_key: Option<String>,
    pub srcdest: String,
    pub logdest: String,
    pub pkgbuild: String,
    pub builds: std::collections::HashMap<super::builder::Arch, BuildConfig>,
    pub s3: Option<S3Config>,
}

#[derive(Debug, serde_derive::Deserialize)]
pub struct BuildConfig {
    pub chroot: String,
}

#[derive(Debug, serde_derive::Deserialize)]
pub struct S3Config {
    pub bucket: String,
    pub region: Region,
}

#[derive(Debug)]
pub struct Region(rusoto_core::Region);

impl<'de> serde::Deserialize<'de> for Region {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Region;

            fn expecting(
                &self,
                formatter: &mut std::fmt::Formatter,
            ) -> Result<(), std::fmt::Error> {
                write!(formatter, "a valid AWS region name")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                use std::error::Error;
                use std::str::FromStr;

                match rusoto_core::Region::from_str(v) {
                    Ok(r) => Ok(Region(r)),
                    Err(e) => Err(serde::de::Error::invalid_value(
                        serde::de::Unexpected::Str(v),
                        &e.description(),
                    )),
                }
            }
        }
        deserializer.deserialize_str(Visitor {})
    }
}

impl Config {
    pub fn from_reader<R>(reader: R) -> serde_yaml::Result<Self>
    where
        R: std::io::Read,
    {
        serde_yaml::from_reader(reader)
    }

    pub fn repo_dir(&self, arch: &super::builder::Arch) -> std::path::PathBuf {
        std::path::PathBuf::from(&self.name)
            .join("os")
            .join(format!("{}", arch))
    }

    pub fn db_path(&self, arch: &super::builder::Arch) -> std::path::PathBuf {
        let mut path = self.repo_dir(arch).join(&self.name).into_os_string();
        path.push(".db");
        std::path::PathBuf::from(path)
    }

    pub fn files_path(&self, arch: &super::builder::Arch) -> std::path::PathBuf {
        let mut path = self.repo_dir(arch).join(&self.name).into_os_string();
        path.push(".files");
        std::path::PathBuf::from(path)
    }

    pub fn abs_path(&self, arch: &super::builder::Arch) -> std::path::PathBuf {
        let mut path = self.repo_dir(arch).join(&self.name).into_os_string();
        path.push(".abs.tar.gz");
        std::path::PathBuf::from(path)
    }

    pub fn package_dir(&self, package_name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(&self.pkgbuild).join(package_name)
    }
}

pub struct S3 {
    client: rusoto_s3::S3Client,
    bucket: String,
}

impl S3 {
    pub fn new(config: &S3Config) -> Self {
        let Region(ref region) = config.region;
        let client = rusoto_s3::S3Client::new(region.clone());
        S3 {
            client,
            bucket: config.bucket.to_owned(),
        }
    }

    pub async fn download_repository(
        &self,
        config: &Config,
        arch: &super::builder::Arch,
    ) -> Result<(), failure::Error> {
        self.get(config.db_path(arch)).await?;
        self.get(config.files_path(arch)).await?;
        self.get(config.abs_path(arch)).await
    }

    pub async fn upload_repository<P>(
        &self,
        config: &Config,
        arch: &super::builder::Arch,
        package_paths: &[P],
    ) -> Result<(), failure::Error>
    where
        P: AsRef<std::path::Path>,
    {
        const XZ_MIME_TYPE: &str = "application/x-xz";
        const SIG_MIME_TYPE: &str = "application/pgp-signature";
        const GZIP_MIME_TYPE: &str = "application/gzip";

        for package_path in package_paths {
            self.put(package_path, XZ_MIME_TYPE).await?;
            if config.package_key.is_some() {
                let mut sig_path = package_path.as_ref().as_os_str().to_os_string();
                sig_path.push(".sig");
                self.put(sig_path, SIG_MIME_TYPE).await?;
            }
        }
        self.put(config.abs_path(arch), GZIP_MIME_TYPE).await?;
        self.put(config.files_path(arch), GZIP_MIME_TYPE).await?;
        let db_path = config.db_path(arch);
        self.put(&db_path, GZIP_MIME_TYPE).await?;
        if config.repo_key.is_some() {
            let mut sig_path = db_path.into_os_string();
            sig_path.push(".sig");
            self.put(sig_path, SIG_MIME_TYPE).await?;
        }
        Ok(())
    }

    async fn get<P>(&self, path: P) -> Result<(), failure::Error>
    where
        P: AsRef<std::path::Path>,
    {
        use rusoto_s3::S3;
        use std::io::Write;

        let path = path.as_ref();
        let mut file = std::fs::File::create(path)?;
        let request = rusoto_s3::GetObjectRequest {
            bucket: self.bucket.to_owned(),
            key: path.to_string_lossy().into_owned(),
            ..rusoto_s3::GetObjectRequest::default()
        };
        println!("Download {}", path.display());
        match self.client.get_object(request).await {
            Ok(output) => {
                if let Some(body) = output.body {
                    use futures::StreamExt;
                    body.for_each(|buf| {
                        let bytes = buf.unwrap();
                        file.write_all(&bytes[..]).unwrap();
                        futures::future::ready(())
                    })
                    .await;
                }
                Ok(())
            }
            Err(rusoto_core::RusotoError::Service(rusoto_s3::GetObjectError::NoSuchKey(_))) => {
                Ok(())
            }
            Err(e) => Err(failure::Error::from(e)),
        }
    }

    async fn put<P>(&self, path: P, content_type: &str) -> Result<(), failure::Error>
    where
        P: AsRef<std::path::Path>,
    {
        use rusoto_s3::S3;
        use std::io::Read;

        let path = path.as_ref();
        // FIXME: need streaming for large files
        // https://github.com/rusoto/rusoto/issues/1028#issuecomment-385361094
        let mut file = std::fs::File::open(path)?;
        let mut body = vec![];
        file.read_to_end(&mut body)?;
        let request = rusoto_s3::PutObjectRequest {
            bucket: self.bucket.to_owned(),
            key: path.to_string_lossy().into_owned(),
            content_type: Some(content_type.to_owned()),
            body: Some(body.into()),
            ..Default::default()
        };
        println!("Upload {}", path.display());
        self.client.put_object(request).await?;
        Ok(())
    }
}
