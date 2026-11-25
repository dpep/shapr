use serde::Serialize;

mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Row {
        /// The row's key.
        id: u32,
    }
}

mod archive {
    use codec::*;

    #[derive(Serialize)]
    struct Entry {
        /// The entry's key.
        id: u32,
    }
}
