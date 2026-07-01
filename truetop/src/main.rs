//! truetop - thin binary entrypoint over the `truetop` library.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    truetop::run().await
}
