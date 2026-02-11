# Technical Documentation Writing

Clear, practical documentation for APIs, README, architecture decisions, and changelogs.

## Principles

1. **Reader-first** - Assume no prior knowledge
2. **Example-driven** - Show, don't just tell
3. **Concise** - Eliminate redundancy
4. **Current** - Update with code changes
5. **Searchable** - Clear headings and keywords

## Code Documentation (Rust)

```rust
/// Brief one-line description.
///
/// Detailed explanation including:
/// - Usage scenarios
/// - Important notes
///
/// # Arguments
/// * `param` - Parameter description and valid range
///
/// # Returns
/// Return value description
///
/// # Errors
/// * `ErrorType` - When this error occurs
///
/// # Examples
/// ```rust
/// let result = function(arg);
/// assert!(result.is_ok());
/// ```
pub fn function(param: Type) -> Result<T, E> { ... }
```

## README Structure

```markdown
# Project Name
One-line description.

## Features
## Quick Start
### Installation
### Usage
## Configuration
## API Documentation
## Contributing
## License
```

## Architecture Decision Records (ADR)

```markdown
# ADR-001: Decision Title

## Status
Accepted | Deprecated | Superseded by ADR-XXX

## Context
Why this decision is needed.

## Decision
What we decided and why.

## Consequences
### Positive
### Negative
### Neutral
```

## CHANGELOG Format

```markdown
# Changelog

## [Unreleased]
### Added
### Changed
### Fixed
### Removed
### Security

## [1.0.0] - 2024-01-01
```

## Documentation Workflow

1. **Analysis**: Gather information from codebase (public APIs, types, errors)
2. **Synthesis**: Organize by document type, identify gaps
3. **Write**: Create clear, example-driven content

## Documentation Checklist

- [ ] Public APIs documented with doc comments
- [ ] Examples are accurate and runnable
- [ ] Error handling explained
- [ ] Parameters and return values documented
- [ ] Terminology consistent throughout
- [ ] README updated with architecture changes
- [ ] ADR created for significant decisions
- [ ] CHANGELOG updated with releases
