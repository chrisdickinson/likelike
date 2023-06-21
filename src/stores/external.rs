use std::env;
use std::path::PathBuf;

use crate::{processors::LinkReadProcessor, Link, LinkReader, LinkWriter};

/// An external store is used for data associated with the link
/// that we are unlikely to use when exporting static site data, especially
/// when that data is large or requires computation. This includes the original source data and text
/// extractions.
pub struct ExternalWrap<T> {
    cache_directory: PathBuf,
    inner: T,
}

impl<T> ExternalWrap<T> {
    pub fn wrap(inner: T) -> Self {
        let default_dir = env::var("LIKELIKE_CACHE_DIR").ok().unwrap_or_else(|| {
            dirs::data_local_dir()
                .map(|mut xs| {
                    xs.push("likelike");
                    xs.push("cache");
                    std::fs::create_dir_all(&xs).expect("Must be able to create XDG_SHARE_HOME");
                    xs.to_string_lossy().to_string()
                })
                .unwrap_or_else(|| "likelike_cache".to_string())
        });

        Self::new(default_dir.into(), inner)
    }

    pub fn new(cache_directory: PathBuf, inner: T) -> Self {
        Self {
            cache_directory,
            inner,
        }
    }
}

#[async_trait::async_trait]
impl<T> LinkReadProcessor for ExternalWrap<T>
where
    T: Send + Sync + LinkReader,
{
    type Inner = T;

    async fn hydrate(&self, mut link: Link) -> eyre::Result<Link> {
        if link.src.is_none() {
            link.src = cacache::read(
                self.cache_directory.as_path(),
                format!("src!{}", link.url()),
            )
            .await
            .ok();
        }

        if link.extracted_text.is_none() {
            link.extracted_text = cacache::read(
                self.cache_directory.as_path(),
                format!("txt!{}", link.url()),
            )
            .await
            .ok()
            .map(|xs| String::from_utf8_lossy(xs.as_slice()).to_string());
        }

        Ok(link)
    }

    fn inner(&self) -> &Self::Inner {
        &self.inner
    }
}

#[async_trait::async_trait]
impl<T: LinkWriter + Send + Sync> LinkWriter for ExternalWrap<T> {
    async fn write(&self, mut link: Link) -> eyre::Result<bool> {
        if let Some(src) = link.src.take() {
            cacache::write(
                self.cache_directory.as_path(),
                format!("src!{}", link.url()),
                src,
            )
            .await?;
        }

        if let Some(extracted_text) = link.extracted_text.take() {
            cacache::write(
                self.cache_directory.as_path(),
                format!("txt!{}", link.url()),
                extracted_text,
            )
            .await?;
        }
        self.inner.write(link).await
    }
}
