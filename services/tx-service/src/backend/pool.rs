use std::{
    collections::BTreeSet,
    fmt::Debug,
    hash::Hash,
    pin::Pin,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

use super::Status;
use crate::{
    backend::{MemPool, MempoolError, RecoverableMempool},
    storage::MempoolStorageAdapter,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolRecoveryState<Key>
where
    Key: Hash + Eq + Ord,
{
    pub pending_items: BTreeSet<Key>,
    pub last_item_timestamp: u64,
}

pub struct Mempool<BlockId, Item, Key, Storage, RuntimeServiceId> {
    pending_items: BTreeSet<Key>,
    last_item_timestamp: u64,
    storage_adapter: Storage,
    _phantom: std::marker::PhantomData<(BlockId, Item, RuntimeServiceId)>,
}

impl<BlockId, Item, Key, Storage, RuntimeServiceId> Debug
    for Mempool<BlockId, Item, Key, Storage, RuntimeServiceId>
where
    BlockId: Debug,
    Item: Debug,
    Key: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mempool")
            .field("pending_items", &self.pending_items)
            .field("last_item_timestamp", &self.last_item_timestamp)
            .field("storage_adapter", &"<StorageAdapter>")
            .finish()
    }
}

#[async_trait]
impl<BlockId, Item, Key, Storage, RuntimeServiceId> MemPool
    for Mempool<BlockId, Item, Key, Storage, RuntimeServiceId>
where
    Key: Hash + Eq + Ord + Clone + Send + Sync + 'static,
    Item: Clone + Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
    BlockId: Hash + Eq + Copy + Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
    Storage:
        MempoolStorageAdapter<RuntimeServiceId, Key = Key, Item = Item> + Send + Sync + 'static,
    Storage::Error: Debug,
    RuntimeServiceId: Send + Sync,
{
    type Settings = ();
    type Item = Item;
    type Key = Key;
    type BlockId = BlockId;
    type Storage = Storage;

    fn new(_settings: Self::Settings, storage: Self::Storage) -> Self {
        Self {
            pending_items: BTreeSet::new(),
            last_item_timestamp: 0,
            storage_adapter: storage,
            _phantom: std::marker::PhantomData,
        }
    }

    async fn add_item<I: Into<Self::Item> + Send>(
        &mut self,
        key: Self::Key,
        item: I,
    ) -> Result<(), MempoolError> {
        if self.pending_items.contains(&key) {
            return Err(MempoolError::ExistingItem);
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        self.storage_adapter
            .store_item(key.clone(), item.into())
            .await
            .map_err(|e| MempoolError::StorageError(format!("{e:?}")))?;

        self.pending_items.insert(key);
        self.last_item_timestamp = timestamp;

        Ok(())
    }

    async fn view(
        &self,
        _ancestor_hint: BlockId,
    ) -> Result<Pin<Box<dyn Stream<Item = Self::Item> + Send>>, MempoolError> {
        let keys: BTreeSet<Key> = self.pending_items.iter().cloned().collect();
        self.get_items_by_keys(keys.into_iter()).await
    }

    async fn get_items_by_keys<I>(
        &self,
        keys: I,
    ) -> Result<Pin<Box<dyn Stream<Item = Self::Item> + Send>>, MempoolError>
    where
        I: IntoIterator<Item = Self::Key> + Send,
    {
        let keys_set: BTreeSet<Self::Key> = keys.into_iter().collect();
        self.storage_adapter
            .get_items(&keys_set)
            .await
            .map_err(|e| MempoolError::StorageError(format!("{e:?}")))
    }

    async fn remove(&mut self, keys: &[Self::Key]) -> Result<(), MempoolError> {
        self.storage_adapter
            .remove_items(keys)
            .await
            .map_err(|e| MempoolError::StorageError(format!("{e:?}")))?;

        for key in keys {
            self.pending_items.remove(key);
        }

        Ok(())
    }

    fn pending_item_count(&self) -> usize {
        self.pending_items.len()
    }

    fn last_item_timestamp(&self) -> u64 {
        self.last_item_timestamp
    }

    fn status(&self, items: &[Self::Key]) -> Vec<Status> {
        items
            .iter()
            .map(|key| {
                if self.pending_items.contains(key) {
                    Status::Pending
                } else {
                    Status::Unknown
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use async_trait::async_trait;

    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
    struct TestKey(u64);

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct TestItem(String);

    #[derive(Debug)]
    struct MockError(String);

    impl fmt::Display for MockError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "MockError({})", self.0)
        }
    }

    struct MockStorageAdapter {
        should_fail_store: bool,
        should_fail_remove: bool,
    }

    impl MockStorageAdapter {
        fn new_ok() -> Self {
            Self {
                should_fail_store: false,
                should_fail_remove: false,
            }
        }

        fn new_fail_store() -> Self {
            Self {
                should_fail_store: true,
                should_fail_remove: false,
            }
        }

        fn new_fail_remove() -> Self {
            Self {
                should_fail_store: false,
                should_fail_remove: true,
            }
        }
    }

    #[async_trait]
    impl MempoolStorageAdapter<()> for MockStorageAdapter {
        type Backend = lb_storage_service::backends::rocksdb::RocksBackend;
        type Item = TestItem;
        type Key = TestKey;
        type Error = MockError;

        fn new(
            _storage_relay: overwatch::services::relay::OutboundRelay<
                <lb_storage_service::StorageService<Self::Backend, ()> as overwatch::services::ServiceData>::Message,
            >,
        ) -> Self {
            unimplemented!("Tests construct MockStorageAdapter directly")
        }

        async fn store_item(
            &mut self,
            _key: Self::Key,
            _item: Self::Item,
        ) -> Result<(), Self::Error> {
            if self.should_fail_store {
                return Err(MockError("store failed".into()));
            }
            Ok(())
        }

        async fn get_items(
            &self,
            _keys: &BTreeSet<Self::Key>,
        ) -> Result<Pin<Box<dyn Stream<Item = Self::Item> + Send>>, Self::Error> {
            Ok(Box::pin(futures::stream::empty()))
        }

        async fn remove_items(&mut self, _keys: &[Self::Key]) -> Result<(), Self::Error> {
            if self.should_fail_remove {
                return Err(MockError("remove failed".into()));
            }
            Ok(())
        }
    }

    type TestMempool = Mempool<u64, TestItem, TestKey, MockStorageAdapter, ()>;

    #[tokio::test]
    async fn add_item_returns_error_on_storage_failure() {
        let adapter = MockStorageAdapter::new_fail_store();
        let mut pool = TestMempool::new((), adapter);

        let result = pool.add_item(TestKey(1), TestItem("test".into())).await;

        let err = result.expect_err("should fail on storage error");
        assert!(matches!(err, MempoolError::StorageError(msg) if msg.contains("store failed")));

        assert_eq!(pool.pending_item_count(), 0);
        assert!(!pool.pending_items.contains(&TestKey(1)));
    }

    #[tokio::test]
    async fn add_item_succeeds_and_tracks_pending() {
        let adapter = MockStorageAdapter::new_ok();
        let mut pool = TestMempool::new((), adapter);

        pool.add_item(TestKey(1), TestItem("test".into()))
            .await
            .expect("add_item should succeed");
        assert_eq!(pool.pending_item_count(), 1);
        assert!(pool.pending_items.contains(&TestKey(1)));
        assert!(pool.last_item_timestamp() > 0);
    }

    #[tokio::test]
    async fn add_item_rejects_duplicate_key() {
        let adapter = MockStorageAdapter::new_ok();
        let mut pool = TestMempool::new((), adapter);

        pool.add_item(TestKey(1), TestItem("first".into()))
            .await
            .unwrap();
        let result = pool.add_item(TestKey(1), TestItem("second".into())).await;

        assert!(matches!(result.unwrap_err(), MempoolError::ExistingItem));
    }

    #[tokio::test]
    async fn remove_returns_error_on_storage_failure() {
        let adapter = MockStorageAdapter::new_fail_remove();
        let mut pool = TestMempool::new((), adapter);

        pool.pending_items.insert(TestKey(1));

        let result = pool.remove(&[TestKey(1)]).await;

        let err = result.expect_err("should fail on storage error");
        assert!(matches!(err, MempoolError::StorageError(msg) if msg.contains("remove failed")));

        assert!(pool.pending_items.contains(&TestKey(1)));
        assert_eq!(pool.pending_item_count(), 1);
    }

    #[tokio::test]
    async fn remove_succeeds_and_clears_pending() {
        let adapter = MockStorageAdapter::new_ok();
        let mut pool = TestMempool::new((), adapter);

        pool.add_item(TestKey(1), TestItem("test".into()))
            .await
            .unwrap();
        assert_eq!(pool.pending_item_count(), 1);

        pool.remove(&[TestKey(1)])
            .await
            .expect("remove should succeed");
        assert_eq!(pool.pending_item_count(), 0);
        assert!(!pool.pending_items.contains(&TestKey(1)));
    }
}

impl<BlockId, Item, Key, Storage, RuntimeServiceId> RecoverableMempool
    for Mempool<BlockId, Item, Key, Storage, RuntimeServiceId>
where
    Key: Hash + Eq + Ord + Clone + Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
    Item: Clone + Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
    BlockId: Hash + Eq + Copy + Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>,
    Storage:
        MempoolStorageAdapter<RuntimeServiceId, Key = Key, Item = Item> + Send + Sync + 'static,
    Storage::Error: Debug,
    RuntimeServiceId: Send + Sync,
{
    type RecoveryState = PoolRecoveryState<Key>;

    fn save(&self) -> Self::RecoveryState {
        PoolRecoveryState {
            pending_items: self.pending_items.clone(),
            last_item_timestamp: self.last_item_timestamp,
        }
    }

    fn recover(
        _settings: <Self as MemPool>::Settings,
        state: Self::RecoveryState,
        storage: <Self as MemPool>::Storage,
    ) -> Self {
        Self {
            pending_items: state.pending_items,
            last_item_timestamp: state.last_item_timestamp,
            storage_adapter: storage,
            _phantom: std::marker::PhantomData,
        }
    }
}
