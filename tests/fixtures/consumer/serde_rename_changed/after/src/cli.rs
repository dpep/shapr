#[derive(Debug, Serialize)]
struct CaseResult {
    rule: String,
    #[serde(rename = "case_number")]
    number: usize,
}
