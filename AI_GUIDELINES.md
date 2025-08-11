# AI Assistant Guidelines

## Code Style and Conventions

### Rust
- **Dependencies**: Always maintain alphabetical order within dependency blocks in Cargo.toml files
- **Error Handling**: Use `expect()` with descriptive messages instead of `unwrap()`
- **Testing**: Use `cargo test -- --nocapture` to see println! output during tests
- **Formatting**: Always run `cargo fmt` before any commit to ensure consistent code formatting
- **Proc Macros**: Use proc macros through their parent crate (e.g., `micromegas_tracing::prelude::*`) rather than importing proc macro crates directly
- **Prelude Imports**: Always use `prelude::*` when importing from a prelude module

### General
- Follow existing code conventions and patterns in the codebase
- Check for existing libraries/frameworks before assuming availability
- Maintain security best practices - never expose secrets or keys
- Use existing utilities and patterns found in neighboring files

## Project Structure
- Main Cargo.toml is located at `rust/Cargo.toml`
- Run cargo commands from the `rust/` directory
- Workspace dependencies should be added to the root Cargo.toml

## Testing
- Always run tests after making changes to verify functionality
