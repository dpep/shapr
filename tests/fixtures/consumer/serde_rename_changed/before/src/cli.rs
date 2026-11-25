#[derive(Debug, Serialize)]
struct CaseResult {
    rule: String,
    #[serde(rename = "case")]
    number: usize,
}
