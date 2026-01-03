# ADR-XXXX: [Short Title of the Decision]

**Status:** [Proposed | Accepted | Rejected | Deprecated | Superseded]  
**Date:** YYYY-MM-DD  
**Deciders:** [Names/Roles]  
**Tags:** [tag1, tag2, tag3]

---

## Context

Describe the issue or problem that motivates this decision. Include:

- What problem are we trying to solve?
- What constraints or requirements must we satisfy?
- What is the current state of the system?
- What triggered the need for this decision?
- Link to relevant issues, discussions, or previous ADRs

**Related GitHub Issues:**
- Related to: #[issue-number] ([Issue Title](link))
- Proposed in: #[issue-number] ([ADR Proposal](link))
- Blocked by: #[issue-number]
- Blocks: #[issue-number]

**Example:**
> We need to choose a physical memory allocator for Phase 1. The allocator must work in a `no_std` environment, support NUMA-aware allocation, and have predictable performance characteristics. Current options include bitmap allocators, buddy allocators, and slab allocators. This decision was proposed in issue #XXX.

---

## Decision

State the architectural decision that was made. Be clear and concise.

**Decision:**
> We will use a [specific solution/approach] because [brief rationale].

Include:
- What was chosen
- Why it was chosen (brief summary; detailed rationale in "Rationale" section)
- Key design decisions or tradeoffs made

---

## Rationale

Explain the reasoning behind the decision. This is the "why" section.

Consider:
- Alignment with [CHARTER.md](../CHARTER.md) principles
- Alignment with project goals (memory safety, isolation, performance, etc.)
- Technical merits and tradeoffs
- Implementation complexity
- Maintainability and correctness

**Key Factors:**
- Factor 1: [Explanation]
- Factor 2: [Explanation]
- Factor 3: [Explanation]

**Alignment with Charter Principles:**
- [Which principles are satisfied?]
- [How does this decision support the project's goals?]

---

## Alternatives Considered

List and evaluate alternative approaches that were considered but not chosen.

### Alternative 1: [Name]

**Description:** [Brief description]

**Pros:**
- Advantage 1
- Advantage 2

**Cons:**
- Disadvantage 1
- Disadvantage 2

**Why Not Chosen:** [Brief explanation]

---

### Alternative 2: [Name]

**Description:** [Brief description]

**Pros:**
- Advantage 1
- Advantage 2

**Cons:**
- Disadvantage 1
- Disadvantage 2

**Why Not Chosen:** [Brief explanation]

---

## Consequences

Describe the positive and negative consequences of this decision.

### Positive

- Benefit 1
- Benefit 2
- Benefit 3

### Negative

- Drawback 1
- Drawback 2

### Risks

- Risk 1: [Description and mitigation]
- Risk 2: [Description and mitigation]

### Implementation Notes

- Implementation detail 1
- Implementation detail 2
- Things to watch out for during implementation

---

## Safety and Security Considerations

For decisions involving unsafe code, security, or safety-critical paths:

**Unsafe Code:**
- Where will unsafe code be needed?
- What safety invariants must be maintained?
- How will safety be verified?

**Security Implications:**
- Security considerations
- Threat model implications
- Capability system interactions (if applicable)

---

## Performance Considerations

For performance-sensitive decisions:

- Expected performance characteristics
- Performance tradeoffs made
- Benchmarking or measurement plans
- Performance targets

---

## Dependencies

**Depends on:**
- ADR-XXXX: [Related decision]
- Feature/component X
- External dependency Y

**Blocks:**
- Feature/component that requires this decision
- Future work enabled by this decision

---

## Implementation Plan

High-level implementation steps (if applicable):

- [ ] Step 1: [Description]
- [ ] Step 2: [Description]
- [ ] Step 3: [Description]

---

## References

Link to relevant documents, discussions, and resources:

**Project Documents:**
- [ARCHITECTURE.md](../ARCHITECTURE.md) - System architecture
- [CHARTER.md](../CHARTER.md) - Design principles
- [MEMORY_ARCHITECTURE.md](../MEMORY_ARCHITECTURE.md) - Related subsystem design (if applicable)
- [ADR README](README.md) - ADR guidelines and process

**GitHub Issues:**
- Proposal: #[issue-number] ([ADR Proposal](https://github.com/iamvirul/ferrous-kernel/issues/XXX))
- Discussion: #[issue-number] ([Discussion thread](https://github.com/iamvirul/ferrous-kernel/issues/XXX))
- Implementation: #[issue-number] ([Implementation tracking](https://github.com/iamvirul/ferrous-kernel/issues/XXX))

**External References:**
- [Research paper or article](link)
- [Related work or inspiration](link)
- [Technical documentation](link)

---

## Notes

Additional notes, open questions, or future considerations:

- Note 1
- Note 2
- Future work that may affect this decision

---

## Status History

Track the status of this ADR over time:

- YYYY-MM-DD: Created (Status: Proposed)
- YYYY-MM-DD: Accepted (Status: Accepted)
- YYYY-MM-DD: Updated (Status: [New Status])

---

## Supersedes / Superseded By

If this ADR supersedes a previous decision:
- **Supersedes:** ADR-XXXX: [Previous Decision Title]

If this ADR is superseded by a newer decision:
- **Superseded by:** ADR-XXXX: [Newer Decision Title]

---

## Approval

- **Proposed by:** [Name] on YYYY-MM-DD
- **Accepted by:** [Name/Role] on YYYY-MM-DD
- **Reviewed by:** [Names] on YYYY-MM-DD

