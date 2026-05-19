use pgevolve_core_macros::Diff;

#[derive(Diff)]
struct S {
    #[diff(skip, nested)]
    name: String,
}

fn main() {}
