//! z3rno-cli: the standalone z3rno CLI binary. Currently a scaffold.

fn version_line() -> String {
    format!("z3rno {}", env!("CARGO_PKG_VERSION"))
}

fn main() {
    println!("{}", version_line());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_has_prefix() {
        assert!(version_line().starts_with("z3rno "));
    }
}
