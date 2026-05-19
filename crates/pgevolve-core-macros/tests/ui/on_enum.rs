use pgevolve_core_macros::Diff;

#[derive(Diff)]
enum E {
    A,
    B,
}

fn main() {}
