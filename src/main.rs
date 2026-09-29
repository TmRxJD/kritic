use kritic::config::Config;
use kritic::server::Kritic;
use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let gov = Kritic::new(Config::from_env());
    let cfg = Config::from_env();
    tokio::spawn(kritic::events::serve(cfg.ws_port, cfg.log_path.clone()));
    gov.warm();
    let service = gov.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    kritic::events::cleanup(&cfg.log_path);
    Ok(())
}
