use serde::Serialize;

#[derive(Debug, Serialize)]
struct Report {
    /// The report's title.
    name: String,
    /// Labels, left out when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
}
