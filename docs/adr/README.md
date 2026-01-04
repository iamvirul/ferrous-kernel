# Architecture Decision Records (ADRs)

This directory contains Architecture Decision Records (ADRs) for Ferrous Kernel. ADRs document important architectural decisions, their context, and consequences.

---

## What is an ADR?

An Architecture Decision Record is a document that captures an important architectural decision made along with its context and consequences. ADRs help:

- **Document decisions** for future reference
- **Understand rationale** behind design choices
- **Track alternatives** that were considered
- **Communicate decisions** to the team
- **Learn from the past** when revisiting decisions

---

## When to Create an ADR?

Create an ADR for:

- **Changes to core abstractions** (memory, scheduling, IPC, capabilities)
- **New subsystems or major components**
- **Performance vs. safety tradeoffs**
- **Security-critical designs**
- **Public API changes**
- **Major design choices** that affect the architecture

**Don't create an ADR for:**
- Minor implementation details
- Bug fixes
- Simple feature additions that don't change architecture
- Decisions that are clearly documented elsewhere

---

## ADR Process

1. **Propose**: Create a GitHub issue using the [ADR Proposal template](../../.github/ISSUE_TEMPLATE/adr-proposal.md) OR write ADR proposal using the [template](template.md)
2. **Discuss**: Review in GitHub issue or pull request, align with [CHARTER.md](../CHARTER.md) principles
3. **Decide**: Accept, reject, or modify the ADR (update issue/ADR status)
4. **Document**: Create ADR document from template, link from GitHub issue
5. **Update**: Update ADR status as implementation progresses, link implementation issues/PRs

---

## ADR Naming Convention

ADRs are numbered sequentially and given descriptive titles:

- **Format**: `ADR-XXXX-short-title.md`
- **Numbering**: Sequential (0001, 0002, 0003, ...)
- **Title**: Short, descriptive title (kebab-case)
- **Example**: `ADR-0001-physical-memory-allocator-choice.md`

---

## ADR Status

ADRs have the following statuses:

- **Proposed**: Under discussion, not yet accepted
- **Accepted**: Decision made, ready for implementation
- **Rejected**: Decision was not accepted (alternative chosen)
- **Deprecated**: Decision is no longer relevant or was replaced
- **Superseded**: Replaced by a newer ADR

---

## Using the Template

1. Copy `template.md` to `ADR-XXXX-short-title.md`
2. Fill in all sections (remove "Example:" placeholders)
3. Set initial status to "Proposed"
4. Submit for review
5. Update status as decision progresses

---

## Index of ADRs

| Number | Title | Status | Date |
|--------|-------|--------|------|
| ADR-XXXX | [Title] | [Status] | YYYY-MM-DD |

*(ADRs will be listed here as they are created)*

---

## Related Documents

- [CHARTER.md](../CHARTER.md) - Design principles and project goals
- [ARCHITECTURE.md](../ARCHITECTURE.md) - System architecture overview

---

## References

- [ADR GitHub Repository](https://github.com/joelparkerhenderson/architecture-decision-record) - ADR format and best practices
- [Documenting Architecture Decisions](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions) - Michael Nygard's original ADR blog post

---

**Last Updated:** 2026-01-04

