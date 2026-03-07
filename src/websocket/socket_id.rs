use rand::RngExt;

/// Generate a Pusher-compatible socket ID: `"{rand}.{rand}"`.
/// Each part is a random number in [0, 10_000_000_000).
pub fn generate() -> String {
    let mut rng = rand::rng();
    let a: u64 = rng.random_range(0..10_000_000_000);
    let b: u64 = rng.random_range(0..10_000_000_000);
    format!("{}.{}", a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_socket_id_format() {
        let id = generate();
        let parts: Vec<&str> = id.split('.').collect();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].parse::<u64>().is_ok());
        assert!(parts[1].parse::<u64>().is_ok());
    }

    #[test]
    fn test_socket_id_range() {
        for _ in 0..100 {
            let id = generate();
            let parts: Vec<&str> = id.split('.').collect();
            let a: u64 = parts[0].parse().unwrap();
            let b: u64 = parts[1].parse().unwrap();
            assert!(a < 10_000_000_000);
            assert!(b < 10_000_000_000);
        }
    }

    #[test]
    fn test_socket_id_unique() {
        let id1 = generate();
        let id2 = generate();
        // Technically could collide, but astronomically unlikely
        assert_ne!(id1, id2);
    }
}
