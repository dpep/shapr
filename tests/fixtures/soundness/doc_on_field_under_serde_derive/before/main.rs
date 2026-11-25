use serde::Serialize;

#[derive(Debug, Serialize)]
struct Report {
    name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
}
