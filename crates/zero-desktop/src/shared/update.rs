use anyhow::{Context, Result};
use custom_utils::updater::UpdateConfig;

pub fn run_update(repo_owner: &str, repo_name: &str, app: &str, force: bool) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio rt")?;
    let outcome = rt.block_on(async {
        UpdateConfig::new(repo_owner, repo_name, env!("CARGO_PKG_VERSION"))
            .bin_name(app)
            .force(force)
            .execute()
            .await
    })?;
    log::info!("update outcome: {outcome:?}");
    Ok(())
}
