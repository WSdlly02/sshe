use clap::Parser;
use mimalloc::MiMalloc;
use sshe::sshed;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = sshed::args::Args::parse();
    // Your sshed implementation here
    println!("Running sshed with args: {:?}", args);
    Ok(())
}
