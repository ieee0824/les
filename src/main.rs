use les::{Les, server};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address = std::env::var("LES_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:9325".into());
    let path = std::env::var("LES_DATABASE_PATH").unwrap_or_else(|_| "les.sqlite".into());
    let app = server::router(Les::open(path)?);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    eprintln!("LES listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
