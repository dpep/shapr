use schemars::JsonSchema;
use serde::Serialize;

#[derive(Serialize, JsonSchema)]
struct Report {
    /// The report's title.
    name: String,
}
