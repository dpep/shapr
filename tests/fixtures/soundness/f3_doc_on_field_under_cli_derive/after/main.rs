#[derive(Parser)]
struct Args {
    /// Who to greet
    #[arg(long)]
    name: String,
}
