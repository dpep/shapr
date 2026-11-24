/// Per-layer cap on candidates pulled from the store before ranking. Exact and
/// prefix matches are guaranteed in full (see `Store::search_candidates`); this
/// only bounds the broad first-char-anchor and trigram-fuzzy recall layers.
/// Scoring is linear and cheap, so this sits well under the latency budget.
const CANDIDATE_LIMIT: usize = 8000;
