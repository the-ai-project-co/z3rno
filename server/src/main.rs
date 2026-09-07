//! z3rno-server: will become an Axum HTTP API. Currently a scaffold.

fn placeholder() -> bool {
    true
}

fn main() {
    if placeholder() {
        println!("z3rno-server: placeholder startup, no server running yet");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_is_true() {
        assert!(placeholder());
    }
}
