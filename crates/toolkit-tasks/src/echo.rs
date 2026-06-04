use crate::kind::{TaskCtx, TaskKind};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct EchoInput {
    pub message: String,
    pub delay_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EchoOutput {
    pub message: String,
}

pub struct EchoTask;

#[async_trait]
impl TaskKind for EchoTask {
    type Input = EchoInput;
    type Output = EchoOutput;
    const KIND: &'static str = "echo";

    async fn run(input: EchoInput, _ctx: TaskCtx) -> Result<EchoOutput> {
        tokio::time::sleep(std::time::Duration::from_millis(input.delay_ms)).await;
        Ok(EchoOutput {
            message: input.message,
        })
    }
}
