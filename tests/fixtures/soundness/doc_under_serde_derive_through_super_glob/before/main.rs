use serde::Serialize;

mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Row {
        id: u32,
    }
}

mod archive {
    use codec::*;

    #[derive(Serialize)]
    struct Entry {
        id: u32,
    }
}
