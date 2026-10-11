#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pisper_server::run_cli().await
}
