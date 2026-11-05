// `sqlx::migrate!` embeds the migrations at compile time; rebuild when a
// file is added, not only when an existing one changes.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
