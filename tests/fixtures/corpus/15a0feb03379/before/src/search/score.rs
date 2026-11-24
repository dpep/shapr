pub struct Boosts {
    /// Behavioral signal: results chosen before for this query.
    pub learned: f64,
    /// Git/filesystem signal: symbols in recently-modified files.
    pub recency: f64,
    /// Branch signal: symbols in files you're changing on this branch (or their
    /// directory neighbors) — where you're most likely working.
    pub branch: f64,
}
