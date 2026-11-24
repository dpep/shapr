#[derive(Parser)]
struct Args {
    /// Name of the person
    #[arg(long)]
    name: String,
}
