# Rust Development Excellence

Comprehensive guide for professional Rust development combining implementation expertise, architectural depth, and project standards.

## Core Principles

### Strategic Programming
Every design decision must answer: **"Does this simplify system understanding, modification, and reasoning?"**

- **Build Deep Modules**: Minimize public API surface, ruthlessly hide implementation details
- **Type System as Shield**: Make illegal states unrepresentable using Newtype patterns and discriminated unions
- **Trait-Oriented Design**: Program to interfaces (small, focused, composable), use `async-trait` for trait objects
- **Error Handling Strategy**:
  - Libraries: FORBIDDEN `unwrap()`/`expect()`, use `thiserror`
  - Applications: use `anyhow` for context
  - Design errors out of existence when possible (e.g., delete non-existent file = no-op)
- **Lifecycle Complexity**: Push lifetimes into implementation, complex signatures in public APIs = RED FLAG

### Code Quality Standards

**Justfile Protocol** (mandatory):
- Single source of truth: **NO justfile in subdirectories**
- Always use `just` when available
- Standard commands: `fmt-all`, `clippy-all`, `test-all`, `check-han` (Chinese character detection)

**Documentation**:
- Write `///` docs BEFORE implementation
- Include runnable `# Examples`
- Document errors with `# Errors` section

**Code Smells** (refactor immediately):
- Excessive `pub` (shallow modules)
- Complex lifetimes in public APIs
- `panic!/unwrap()/expect()` in libraries
- Excessive `.clone()`
- Boolean parameters (use `enum` instead)

## Development Workflow

### Micro-Loop for Each Logical Unit
1. **Code**: Minimal functional increment
2. **Format**: `just fmt-all`
3. **Lint**: `just clippy-all` (ALL warnings must be fixed)
4. **Test**: `just test-all`, write necessary tests
5. **Review**: `git diff` self-review
6. **Commit**: Conventional Commits with motivation/implementation details

### Commit Protocol
- Format: `type(scope): description`
- Body: `[MOTIVATION]`, `[IMPLEMENTATION DETAILS]`
- Never use `-S` flag
- Repository config changes: separate commit

## Project Defaults

### Cargo.toml Configuration
```toml
[package]
edition = "2024"
rust-version = "1.85"

[lints.rust]
unsafe_code = "warn"

[lints.clippy]
all = "warn"
pedantic = "warn"
```

### Unsafe Code Rule
Every `unsafe` block MUST have `// SAFETY:` comment explaining why it's safe.

## Code Review Checklist

- [ ] No `unwrap()`/`expect()` in library code
- [ ] No complex lifetimes in public APIs
- [ ] Using `enum` instead of bool flags
- [ ] Appropriate error handling (thiserror/anyhow)
- [ ] Documentation with examples
- [ ] All clippy warnings resolved
- [ ] Tests written for new logic
- [ ] No Chinese characters in code/comments
