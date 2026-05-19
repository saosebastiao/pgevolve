use pgevolve_core_macros::Diff;

#[derive(Diff)]
struct S {
    #[diff(rename = "x")]
    name: String,
}

fn main() {}
