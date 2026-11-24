#[get("/<id>")]
fn show(id: i32) -> i32 {
    id + 1
}
