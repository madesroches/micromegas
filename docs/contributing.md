# Contributing to Micromegas

We welcome contributions to the Micromegas project! This guide will help you get started with contributing code, documentation, or reporting issues.

## Code of Conduct

By participating in this project, you agree to maintain a respectful and inclusive environment for all contributors.

## Getting Started

### Development Setup

1. **Clone the repository**:
   ```bash
   git clone https://github.com/madesroches/micromegas.git
   cd micromegas
   ```

2. **Set up development environment**:
   Follow the [Getting Started Guide](getting-started.md) to set up your local environment.

3. **Install development dependencies**:
   ```bash
   # Rust development
   cd rust
   cargo build
   
   # Python development
   cd python/micromegas
   poetry install
   
   # Documentation
   pip install -r docs/docs-requirements.txt
   ```

## Contributing Code

### Before You Start

1. **Check existing issues** on [GitHub Issues](https://github.com/madesroches/micromegas/issues)
2. **Open an issue** to discuss your proposed changes if it's a significant feature
3. **Fork the repository** and create a feature branch

### Development Workflow

1. **Create a feature branch**:
   ```bash
   git checkout -b feature/your-feature-name
   # or
   git checkout -b bugfix/issue-description
   ```

2. **Follow coding standards** (see [AI Guidelines](AI_GUIDELINES.md)):
   - **Rust**: Run `cargo fmt` before any commit
   - **Python**: Use `black` for formatting
   - **Dependencies**: Keep alphabetical order in Cargo.toml
   - **Error handling**: Use `expect()` with descriptive messages instead of `unwrap()`

3. **Write tests**:
   ```bash
   # Rust tests
   cd rust
   cargo test
   
   # Python tests
   cd python/micromegas
   pytest
   ```

4. **Run CI pipeline locally**:
   ```bash
   cd rust
   python3 ../build/rust_ci.py
   ```

5. **Commit with clear messages**:
   ```bash
   git commit -m "Add histogram generation for span duration analysis"
   ```
   
   **Note**: Never include AI-generated credits or co-author tags in commit messages.

6. **Push and create Pull Request**:
   ```bash
   git push origin feature/your-feature-name
   ```
   
   Before creating a PR, run:
   ```bash
   git log --oneline main..HEAD
   ```
   to list all commits in your branch.

### Code Review Process

1. **Automated checks**: Ensure all CI checks pass
2. **Code review**: Maintainers will review your changes
3. **Address feedback**: Make requested changes if needed
4. **Merge**: Once approved, your PR will be merged

## Contributing Documentation

### MkDocs Documentation

The main documentation uses MkDocs with Material theme:

1. **Edit documentation**:
   - Files are in `docs/` directory
   - Use Markdown format
   - Follow existing structure and style

2. **Preview changes**:
   ```bash
   mkdocs serve
   # Visit http://localhost:8000
   ```

3. **Build documentation**:
   ```bash
   python docs/build-docs.py
   ```

### Documentation Guidelines

- **Clear and concise**: Write for your audience
- **Code examples**: Include working examples with expected output
- **Cross-references**: Link to related sections
- **Consistent formatting**: Follow existing patterns

## Reporting Issues

### Bug Reports

Include the following information:

- **Environment**: OS, Rust version, Python version
- **Micromegas version**: Git commit or release version
- **Steps to reproduce**: Clear, minimal reproduction steps
- **Expected vs actual behavior**: What should happen vs what happens
- **Logs/errors**: Include relevant error messages or logs
- **Configuration**: Relevant environment variables or config

### Feature Requests

- **Use case**: Describe the problem you're trying to solve
- **Proposed solution**: Your suggested approach
- **Alternatives**: Other approaches you've considered
- **Impact**: Who would benefit from this feature

## Development Guidelines

### Architecture Understanding

Familiarize yourself with the [architecture overview](architecture/index.md):

- **High-performance instrumentation** (20ns overhead)
- **Lakehouse architecture** with object storage
- **DataFusion-powered analytics**
- **FlightSQL protocol** for efficient data transfer

### Key Areas for Contribution

1. **Core Rust Libraries**:
   - Tracing instrumentation improvements
   - Analytics engine enhancements
   - Performance optimizations

2. **Services**:
   - Ingestion service features
   - FlightSQL server improvements
   - Admin CLI enhancements

3. **Client Libraries**:
   - Python API improvements
   - New language bindings

4. **Documentation**:
   - Query examples and patterns
   - Performance guidance
   - Integration guides

5. **Testing**:
   - Unit test coverage
   - Integration tests
   - Performance benchmarks

### Performance Considerations

- **Benchmarking**: Include benchmarks for performance-critical changes
- **Memory usage**: Consider memory implications of new features
- **Backwards compatibility**: Maintain API compatibility when possible

### Security

- **No secrets in code**: Never commit API keys, passwords, or tokens
- **Input validation**: Validate all external inputs
- **Dependencies**: Keep dependencies updated and minimal

## Community

### Getting Help

- **Documentation**: Check the [documentation](index.md) first
- **GitHub Issues**: Search existing issues before creating new ones
- **Discussions**: Use GitHub Discussions for questions and ideas

### Stay Updated

- **Watch the repository** for updates
- **Follow releases** for new features and bug fixes
- **Join discussions** about future directions

## Recognition

Contributors are recognized in:
- Git commit history
- Release notes for significant contributions
- Special thanks in documentation

Thank you for contributing to Micromegas! Your contributions help make observability more accessible and cost-effective for everyone.
