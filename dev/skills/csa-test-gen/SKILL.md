---
name: csa-test-gen
description: Comprehensive test design for Rust projects following Core-first TDD and test pyramid principles
allowed-tools: Bash, Read, Grep, Glob, Edit, Write
---

# Test Generation & TDD Excellence

Comprehensive test design for Rust projects following Core-first TDD and test pyramid principles.

## Core-First TDD Workflow

Always test pure logic BEFORE UI:

```
1. Write pure logic in core/ or domain/ module
2. Write unit tests for core logic
3. Verify all tests pass
4. Only THEN implement UI layer
```

## Test Pyramid

Target: 70% unit, 20% integration, 10% E2E

## Unit Test Template (Rust)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Naming: test_<function>_<scenario>_<expected>
    #[test]
    fn test_parse_valid_input_returns_expected() {
        // Arrange
        let input = "valid_input";
        // Act
        let result = parse(input);
        // Assert
        assert_eq!(result, expected_value);
    }

    #[test]
    fn test_parse_empty_input_returns_error() {
        let result = parse("");
        assert!(result.is_err());
    }
}
```

## Test Categories

| Category | Focus | Example |
|----------|-------|---------|
| **Happy Path** | Typical valid input | `parse("valid") -> Ok(value)` |
| **Boundary** | Empty, null, min, max, +/-1 | `parse("") -> Err(...)` |
| **Error** | Invalid input, not found, denied | `parse("@#$") -> Err(...)` |
| **Concurrent** | Race conditions, deadlock | Multiple tasks with shared state |

## Mocking Strategy

```rust
// Define trait abstraction
pub trait UserRepository {
    fn find(&self, id: &str) -> Option<User>;
}

// Test mock
#[cfg(test)]
struct MockUserRepository {
    users: HashMap<String, User>,
}

#[cfg(test)]
impl UserRepository for MockUserRepository {
    fn find(&self, id: &str) -> Option<User> {
        self.users.get(id).cloned()
    }
}
```

## Property-Based Testing

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_serialize_roundtrip(value in any::<MyType>()) {
        let serialized = serde_json::to_string(&value).unwrap();
        let deserialized: MyType = serde_json::from_str(&serialized).unwrap();
        prop_assert_eq!(value, deserialized);
    }
}
```

## Fuzzing

```rust
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        let _ = parse(input); // Should never panic
    }
});
```

## Test Checklist

- [ ] Happy path cases covered
- [ ] All boundary conditions tested
- [ ] Error handling verified
- [ ] Clear, descriptive test names
- [ ] No test interdependencies (order-independent)
- [ ] Mocks isolate external dependencies
- [ ] Assertions verify meaningful state
- [ ] Async tests with `#[tokio::test]`
- [ ] Tests self-clean (no fixtures left behind)
- [ ] Fast execution (<100ms per unit test)
