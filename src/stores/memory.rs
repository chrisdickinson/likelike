use futures::{stream, Stream};

use std::{collections::HashMap, fmt::Debug, pin::Pin};
use tokio::sync::Mutex;

use crate::{Link, LinkReader, LinkWriter};

/// An in-memory link store.
#[derive(Default, Debug)]
struct InMemoryStore {
    data: Mutex<HashMap<String, Link>>,
}

impl InMemoryStore {
    fn new() -> Self {
        Self {
            ..Default::default()
        }
    }
}

#[async_trait::async_trait]
impl LinkReader for InMemoryStore {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>> {
        let data = self.data.lock().await;

        Ok(data.get(link).cloned())
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a + Send>>> {
        let data = self.data.lock().await;

        // This "collect()" seems to be doing something for us, since implementing clippy's suggestion
        // nets us an E0597 lifetime error.
        #[allow(clippy::needless_collect)]
        let values: Vec<_> = data.values().cloned().collect();

        Ok(Box::pin(stream::iter(values.into_iter())))
    }
}

#[async_trait::async_trait]
impl LinkWriter for InMemoryStore {
    async fn write(&self, link: Link) -> eyre::Result<bool> {
        let mut data = self.data.lock().await;
        data.insert(link.url.clone(), link);
        Ok(true)
    }
}
