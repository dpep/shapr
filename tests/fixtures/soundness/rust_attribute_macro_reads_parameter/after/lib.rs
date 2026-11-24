#[get("/<id>")]
fn show(key: i32) -> i32 {
    key + 1
}
