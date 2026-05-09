//! `pgevolve` CLI entry point.

fn main() -> anyhow::Result<()> {
    println!("pgevolve {} (skeleton)", pgevolve_core::VERSION);
    Ok(())
}
