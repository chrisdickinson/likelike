use chrono::Utc;

use crate::{LinkReadProcessor, LinkReader, LinkWriter, Link};

pub struct TextProcessorWrap<T> {
    inner: T,
}

impl<T> TextProcessorWrap<T> {
    pub fn wrap(inner: T) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl<T> LinkReadProcessor for TextProcessorWrap<T>
where
    T: Send + Sync + LinkReader,
{
    type Inner = T;

    fn inner(&self) -> &Self::Inner {
        &self.inner
    }
}


#[async_trait::async_trait]
impl<T: LinkWriter + Send + Sync> LinkWriter for TextProcessorWrap<T> {
    async fn write(&self, mut link: Link) -> eyre::Result<bool> {
        if link.last_processed().is_none() && link.src().is_some() && link.is_plaintext() {
            link.last_processed = Some(Utc::now());
            link.extracted_text = link.src()
                .and_then(|xs| std::str::from_utf8(xs).ok())
                .map(str::to_string);
        }
        self.inner.write(link).await
    }
}


