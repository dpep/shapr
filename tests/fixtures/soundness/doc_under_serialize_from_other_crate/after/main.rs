use archive::Serialize;

#[derive(Serialize)]
struct Report {
    /// The report's title.
    name: String,
}
