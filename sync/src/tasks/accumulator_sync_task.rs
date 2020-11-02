// Copyright (c) The Starcoin Core Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::tasks::BlockIdFetcher;
use anyhow::{ensure, Result};
use futures::future::BoxFuture;
use futures::FutureExt;
use logger::prelude::*;
use starcoin_accumulator::accumulator_info::AccumulatorInfo;
use starcoin_accumulator::{Accumulator, AccumulatorTreeStore, MerkleAccumulator};
use starcoin_crypto::HashValue;
use starcoin_types::block::BlockNumber;
use std::pin::Pin;
use std::sync::Arc;
use stream_task::{TaskResultCollector, TaskState};

#[derive(Clone)]
pub struct BlockAccumulatorSyncState {
    start_number: BlockNumber,
    target: AccumulatorInfo,
    fetcher: Arc<dyn BlockIdFetcher>,
    batch_size: usize,
}

impl BlockAccumulatorSyncState {
    pub fn new<F>(
        start_number: BlockNumber,
        target: AccumulatorInfo,
        fetcher: F,
        batch_size: usize,
    ) -> Self
    where
        F: BlockIdFetcher + 'static,
    {
        Self {
            start_number,
            target,
            fetcher: Arc::new(fetcher),
            batch_size,
        }
    }
}

impl TaskState for BlockAccumulatorSyncState {
    type Item = HashValue;

    fn new_sub_task(self) -> BoxFuture<'static, Result<Vec<Self::Item>>> {
        async move {
            let start = self.start_number;
            let target = self.target.num_leaves;
            let mut max_size = (target - start) as usize;
            if max_size > self.batch_size {
                max_size = self.batch_size;
            }
            debug!(
                "Accumulator sync task: start_number: {}, target_number: {}",
                start, target
            );
            self.fetcher.fetch_block_ids(start, false, max_size).await
        }
        .boxed()
    }

    fn next(&self) -> Option<Self> {
        let next_start_number = self.start_number + (self.batch_size as u64);
        if next_start_number >= self.target.num_leaves {
            None
        } else {
            Some(Self {
                start_number: next_start_number,
                target: self.target.clone(),
                fetcher: self.fetcher.clone(),
                batch_size: self.batch_size,
            })
        }
    }
}

pub struct AccumulatorCollector {
    accumulator: MerkleAccumulator,
    target: AccumulatorInfo,
}

impl AccumulatorCollector {
    pub fn new(
        store: Arc<dyn AccumulatorTreeStore>,
        current: AccumulatorInfo,
        target: AccumulatorInfo,
    ) -> Self {
        let accumulator = MerkleAccumulator::new_with_info(current, store);
        Self {
            accumulator,
            target,
        }
    }
}

impl TaskResultCollector<HashValue> for AccumulatorCollector {
    type Output = MerkleAccumulator;

    fn collect(self: Pin<&mut Self>, item: HashValue) -> Result<()> {
        self.accumulator.append(&[item])?;
        self.accumulator.flush()
    }

    fn finish(self) -> Result<Self::Output> {
        let info = self.accumulator.get_info();
        ensure!(
            info == self.target,
            "Target accumulator: {:?}, but got: {:?}",
            self.target,
            info
        );
        Ok(self.accumulator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;
    use futures_timer::Delay;
    use pin_utils::core_reexport::time::Duration;
    use starcoin_accumulator::tree_store::mock::MockAccumulatorStore;
    use starcoin_accumulator::MerkleAccumulator;
    use stream_task::TaskGenerator;

    struct MockBlockIdFetcher {
        accumulator: MerkleAccumulator,
    }

    impl MockBlockIdFetcher {
        async fn fetch_block_ids_async(
            &self,
            start_number: u64,
            reverse: bool,
            max_size: usize,
        ) -> Result<Vec<HashValue>> {
            Delay::new(Duration::from_millis(100)).await;
            self.accumulator.get_leaves(start_number, reverse, max_size)
        }
    }

    impl BlockIdFetcher for MockBlockIdFetcher {
        fn fetch_block_ids(
            &self,
            start_number: u64,
            reverse: bool,
            max_size: usize,
        ) -> BoxFuture<Result<Vec<HashValue>>> {
            self.fetch_block_ids_async(start_number, reverse, max_size)
                .boxed()
        }
    }

    #[stest::test]
    async fn test_accumulator_sync_by_stream_task() -> Result<()> {
        let store = Arc::new(MockAccumulatorStore::new());
        let accumulator = MerkleAccumulator::new_empty(store.clone());
        for _i in 0..100 {
            accumulator.append(&[HashValue::random()])?;
        }
        accumulator.flush().unwrap();
        let info0 = accumulator.get_info();
        assert_eq!(info0.num_leaves, 100);
        for _i in 0..100 {
            accumulator.append(&[HashValue::random()])?;
        }
        accumulator.flush().unwrap();
        let info1 = accumulator.get_info();
        assert_eq!(info1.num_leaves, 200);
        let fetcher = MockBlockIdFetcher { accumulator };
        let store2 = MockAccumulatorStore::copy_from(store.as_ref());

        let task_state =
            BlockAccumulatorSyncState::new(info0.num_leaves, info1.clone(), fetcher, 7);
        let collector = AccumulatorCollector::new(Arc::new(store2), info0, info1.clone());

        let sync_task = TaskGenerator::new(task_state, 5, 3, 1, collector).generate();

        let info2 = sync_task.await?.get_info();
        assert_eq!(info1, info2);
        Ok(())
    }
}