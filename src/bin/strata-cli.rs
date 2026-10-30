use clap::Parser;
fn main() -> anyhow::Result<()> {
    let id = strata::cli::enqueue(strata::cli::Cli::parse())?;
    println!("Queued transfer {id} in Strata.");
    Ok(())
}
