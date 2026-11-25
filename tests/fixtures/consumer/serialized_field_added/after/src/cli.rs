#[derive(Debug, Serialize)]
struct CaseResult {
    rule: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rewritten_by: Vec<String>,
}
