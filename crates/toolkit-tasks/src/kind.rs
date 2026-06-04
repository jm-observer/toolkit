use crate::store;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;
use toolkit_core::SqlitePool;

#[async_trait]
pub trait TaskKind: 'static + Send + Sync {
    type Input: Serialize + DeserializeOwned + Send;
    type Output: Serialize + DeserializeOwned + Send;
    const KIND: &'static str;
    async fn run(input: Self::Input, ctx: TaskCtx) -> Result<Self::Output>;
}

#[derive(Clone)]
pub struct TaskCtx {
    pub task_id: String,
    pub pool: SqlitePool,
    /// 数据根目录（与 toolkit-server `--data-dir` 一致），供任务体解析衍生路径。
    pub data_dir: PathBuf,
}

impl TaskCtx {
    /// 任务运行中上报进度（写入 tasks.progress 列）。
    pub fn report_progress(&self, value: Value) -> Result<()> {
        store::update_progress(&self.pool, &self.task_id, &value)
    }
}

#[async_trait]
pub(crate) trait ErasedKind: Send + Sync {
    async fn run_json(&self, input: Value, ctx: TaskCtx) -> Result<Value>;
}

pub(crate) struct KindWrapper<K: TaskKind>(pub PhantomData<K>);

#[async_trait]
impl<K: TaskKind> ErasedKind for KindWrapper<K> {
    async fn run_json(&self, input: Value, ctx: TaskCtx) -> Result<Value> {
        let typed: K::Input =
            serde_json::from_value(input).map_err(|e| anyhow!("invalid input: {e}"))?;
        let out = K::run(typed, ctx).await?;
        Ok(serde_json::to_value(out)?)
    }
}

#[derive(Default)]
pub struct Registry {
    kinds: HashMap<&'static str, Arc<dyn ErasedKind>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<K: TaskKind>(&mut self) {
        self.kinds
            .insert(K::KIND, Arc::new(KindWrapper::<K>(PhantomData)));
    }

    pub fn kinds(&self) -> Vec<&'static str> {
        self.kinds.keys().copied().collect()
    }

    pub(crate) fn get(&self, kind: &str) -> Option<Arc<dyn ErasedKind>> {
        self.kinds.get(kind).cloned()
    }
}
