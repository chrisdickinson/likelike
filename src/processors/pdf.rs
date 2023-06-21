use crate::{processors::LinkReadProcessor, Link, LinkReader, LinkWriter};
use chrono::Utc;

/// An external store is used for data associated with the link
/// that we are unlikely to use when exporting static site data, especially
/// when that data is large or requires computation. This includes the original source data and text
/// extractions.
pub struct PdfProcessorWrap<T> {
    inner: T,
}

impl<T> PdfProcessorWrap<T> {
    pub fn wrap(inner: T) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl<T> LinkReadProcessor for PdfProcessorWrap<T>
where
    T: Send + Sync + LinkReader,
{
    type Inner = T;

    fn inner(&self) -> &Self::Inner {
        &self.inner
    }
}

#[async_trait::async_trait]
impl<T: LinkWriter + Send + Sync> LinkWriter for PdfProcessorWrap<T> {
    async fn write(&self, mut link: Link) -> eyre::Result<bool> {
        if link.last_processed().is_none() && link.src().is_some() && link.is_pdf() {
            link.last_processed = Some(Utc::now());
            link.extracted_text = std::thread::scope(|s| {
                s.spawn(|| {
                    // pdf_extract LOVES to panic
                    std::panic::set_hook(Box::new(|_| {}));

                    pdf_extract::extract_text_from_mem(link.src().unwrap_or(b"")).ok()
                })
                .join()
            })
            .ok()
            .flatten();
        }
        self.inner.write(link).await
    }
}
