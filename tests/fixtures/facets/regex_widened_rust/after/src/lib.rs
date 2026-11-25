use regex::Regex;

pub fn mac() -> Regex {
    Regex::new(r"^[0-9a-f]{1,2}(:[0-9a-f]{1,2}){5}$").unwrap()
}
