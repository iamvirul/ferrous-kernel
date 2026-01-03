# Contributing to Ferrous Kernel

Thank you for your interest in contributing to Ferrous Kernel! This document provides guidelines and instructions for contributing to the project.

---

## Code of Conduct

Ferrous Kernel is committed to providing a welcoming and inclusive environment. All contributors are expected to:

- Be respectful and considerate
- Focus on constructive feedback
- Collaborate openly and honestly
- Follow the project's design principles and guidelines

---

## Getting Started

### Prerequisites

Before contributing, make sure you have:

- Rust toolchain (nightly version)
- QEMU (for testing and running the kernel)
- Cross-compilation tools for x86_64
- Basic understanding of operating systems and Rust systems programming

### Setting Up the Development Environment

1. Fork and clone the repository:
   ```bash
   git clone https://github.com/iamvirul/ferrous-kernel.git
   cd ferrous-kernel
   ```

2. Review the project documentation:
   - [CHARTER.md](CHARTER.md) - Project goals and design principles
   - [ARCHITECTURE.md](ARCHITECTURE.md) - System architecture
   - [ROADMAP.md](ROADMAP.md) - Development phases and milestones
   - [CLAUDE.md](../CLAUDE.md) - Development guidelines

3. Check the current phase and available work:
   - Review the [ROADMAP.md](ROADMAP.md) for current phase deliverables
   - Check open issues for tasks
   - Look for issues labeled `good-first-issue` if you're new to the project

---

## Contribution Process

### 1. Find Something to Work On

- **Issues**: Check the [Issues](https://github.com/iamvirul/ferrous-kernel/issues) page for tasks
- **Documentation**: Documentation improvements are always welcome
- **Architecture**: For major changes, start with an [ADR Proposal](../../.github/ISSUE_TEMPLATE/adr-proposal.md)

### 2. Discuss Your Contribution

- For **major changes** or **architectural decisions**: Create an ADR Proposal issue first
- For **features**: Create a Feature Request issue to discuss approach
- For **bugs**: Create a Bug Report issue
- For **questions**: Open an issue with your question

### 3. Create a Branch

Create a feature branch from `main`:

```bash
git checkout -b feature/your-feature-name
# or
git checkout -b fix/your-fix-name
```

**Branch naming conventions:**
- `feature/` - New features
- `fix/` - Bug fixes
- `docs/` - Documentation changes
- `refactor/` - Code refactoring
- `test/` - Test additions or improvements

### 4. Make Your Changes

**Before writing code:**

1. Review relevant architecture documents
2. Understand the design principles (see [CHARTER.md](CHARTER.md))
3. Check if your change requires an ADR (see [ADR README](adr/README.md))

**While coding:**

- Follow Rust best practices and idioms
- Write clear, understandable code
- Add comments for complex logic
- Document all `unsafe` blocks with safety comments
- Write tests for new code
- Update documentation as needed

### 5. Commit Your Changes

Write clear, descriptive commit messages:

```
Short summary (50 chars or less)

More detailed explanation if needed. Wrap at 72 characters.
Explain the what and why, not the how.

- Bullet points are okay too
- Reference issues: Fixes #123
```

**Commit message guidelines:**
- First line: Brief summary (imperative mood)
- Body: Explain what and why (if needed)
- Reference issues: `Fixes #123` or `Closes #456`
- Sign off on commits (DCO - Developer Certificate of Origin)

### 6. Test Your Changes

- Run existing tests: `cargo test`
- Run linters: `cargo clippy --all-targets -- -D warnings`
- Check formatting: `cargo fmt --check`
- Test in QEMU if applicable

### 7. Submit a Pull Request

1. Push your branch to your fork:
   ```bash
   git push origin feature/your-feature-name
   ```

2. Open a Pull Request on GitHub

3. Fill out the PR template with:
   - Description of changes
   - Related issues
   - Testing performed
   - Any breaking changes

4. Wait for review and address feedback

---

## Code Standards

### Rust Style

- Follow standard Rust formatting: `cargo fmt`
- Follow Rust API guidelines where applicable
- Use `rustfmt` and `clippy` - warnings are errors in CI

### Unsafe Code

**Critical**: All `unsafe` code requires:

1. **Safety comment** explaining:
   - What invariants must hold
   - Why they are guaranteed to hold
   - What could go wrong if violated

2. **Additional review** - Unsafe code gets extra scrutiny

3. **Isolation** - Wrap unsafe code behind safe APIs

See [CLAUDE.md](../CLAUDE.md) for detailed unsafe Rust guidelines.

**Example:**
```rust
/// Map a virtual page to a physical frame.
///
/// # Safety
/// - `virt_addr` must be a valid virtual address in canonical form
/// - `phys_frame` must be a valid physical frame
/// - The virtual address must not already be mapped
/// - The page table structure must be valid
pub unsafe fn map_page(
    &mut self,
    virt_addr: VirtualAddress,
    phys_frame: PhysicalFrame,
    flags: PageFlags,
) -> Result<(), MemoryError> {
    // Implementation...
}
```

### Documentation

- Public APIs must be documented
- Code comments explain "why", not "what"
- Documentation should reflect reality
- Update documentation when changing code

### Testing

- Write unit tests for new functionality
- Write integration tests for major features
- Tests should be deterministic
- Test error cases, not just happy paths

---

## Architecture Decision Records (ADRs)

Major changes require ADRs. See [ADR README](adr/README.md) for guidelines.

**When to create an ADR:**
- Changes to core abstractions (memory, scheduling, IPC, capabilities)
- New subsystems or major components
- Performance vs. safety tradeoffs
- Security-critical designs
- Public API changes

**Process:**
1. Create an [ADR Proposal issue](../../.github/ISSUE_TEMPLATE/adr-proposal.md)
2. Discuss and get approval
3. Create ADR document from [template](adr/template.md)
4. Link ADR from implementation PR

---

## Design Principles

All contributions must align with the project's design principles:

1. **Correctness first. Performance follows. Features last.**
2. **Unsafe Rust is explicit, reviewed, and isolated**
3. **Simplicity beats features**
4. **No silent global state**
5. **Fail fast, fail visibly**

See [CHARTER.md](CHARTER.md) for complete principles.

---

## Review Process

### What to Expect

- **All code is reviewed** - No exceptions
- **Review may take time** - This is a research project with high standards
- **Feedback is constructive** - Focus on improving the code
- **Iteration is normal** - Multiple rounds of review are common

### Review Criteria

Code is evaluated on:

- **Correctness** - Does it work correctly?
- **Safety** - Is it safe (especially unsafe code)?
- **Simplicity** - Is it the simplest solution?
- **Alignment** - Does it align with design principles?
- **Documentation** - Is it well-documented?
- **Testing** - Are there adequate tests?

### Responding to Review

- Address all feedback
- Ask questions if something is unclear
- Be open to suggestions
- Update code based on feedback
- Mark conversations as resolved when addressed

---

## Areas for Contribution

### Current Phase (Phase 0: Foundation & Design)

- Documentation improvements
- Architecture design documents
- ADR proposals and discussions
- Build system setup
- Development tooling

### Future Phases

- Code implementation (Phase 1+)
- Testing infrastructure
- Driver development
- Performance optimization
- Documentation and examples

---

## Getting Help

- **Questions**: Open an issue with the `question` label
- **Discussion**: Use GitHub Issues for technical discussions
- **Architecture**: Create an ADR Proposal issue for architectural questions
- **Bugs**: Create a Bug Report issue

---

## Licensing

By contributing to Ferrous Kernel, you agree that your contributions will be licensed under the Apache License 2.0, the same license as the project (see [LICENSE](../LICENSE)). Apache 2.0 provides explicit patent protection and is widely used in modern open-source projects, including many in the Rust ecosystem.

---

## Recognition

Contributors are recognized in:

- Git commit history
- Release notes (when applicable)
- Project documentation (for significant contributions)

---

## Thank You!

Contributions of all kinds are valued:

- Code contributions
- Documentation improvements
- Bug reports
- Feature suggestions
- Architecture discussions
- Testing and feedback

Thank you for helping build Ferrous Kernel!

---

**Remember**: This is a research project. We're exploring new approaches and learning as we build. Your contributions help advance the state of operating system design.

---

**Questions?** Open an issue or check the [documentation](README.md).

