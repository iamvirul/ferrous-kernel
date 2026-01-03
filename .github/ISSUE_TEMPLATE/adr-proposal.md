---
name: ADR Proposal
about: Propose an Architecture Decision Record (ADR)
title: '[ADR] '
labels: 'architecture, adr'
assignees: ''
---

## ADR Proposal

This issue is for proposing an Architecture Decision Record (ADR). ADRs document important architectural decisions, their context, and consequences.

**See:** [ADR Template](../../docs/adr/template.md) and [ADR README](../../docs/adr/README.md)

---

## Decision Summary

**Brief title:** [One-line summary of the decision]

**Decision area:** [Memory Management | Scheduling | IPC | Capabilities | Security | Performance | Other]

**Status:** Proposed

---

## Context

**What problem are we trying to solve?**
<!-- Describe the issue or problem that motivates this decision -->

**What constraints or requirements must we satisfy?**
<!-- List constraints, requirements, or limitations -->

**What is the current state?**
<!-- Describe the current state of the system -->

**What triggered the need for this decision?**
<!-- What event or discovery led to this decision? -->

---

## Proposed Decision

**What is being proposed?**
<!-- Describe the proposed solution or approach -->

**Why is this being proposed?**
<!-- Brief rationale for the proposal -->

---

## Alternatives Considered

**Alternative 1:** [Name]
- Pros: [Advantages]
- Cons: [Disadvantages]
- Why not chosen: [Reason]

**Alternative 2:** [Name]
- Pros: [Advantages]
- Cons: [Disadvantages]
- Why not chosen: [Reason]

<!-- Add more alternatives as needed -->

---

## Alignment with Charter Principles

**Which principles from [CHARTER.md](../../docs/CHARTER.md) are relevant?**
- [ ] Correctness first
- [ ] Safety and memory safety
- [ ] Simplicity
- [ ] Performance considerations
- [ ] Isolation and fault containment
- [ ] Observability
- [ ] Other: [Specify]

**How does this decision align with the project's goals?**
<!-- Explain alignment with project goals -->

---

## Consequences

**Positive consequences:**
- [Benefit 1]
- [Benefit 2]

**Negative consequences:**
- [Drawback 1]
- [Drawback 2]

**Risks:**
- [Risk 1 and mitigation]
- [Risk 2 and mitigation]

---

## Safety and Security Considerations

**Unsafe code implications:**
<!-- Will this require unsafe code? What safety invariants must be maintained? -->

**Security implications:**
<!-- What are the security considerations? How does this affect the capability model? -->

---

## Performance Considerations

**Expected performance impact:**
<!-- Performance characteristics, tradeoffs, measurement plans -->

---

## Implementation Plan

**High-level steps:**
- [ ] Step 1: [Description]
- [ ] Step 2: [Description]
- [ ] Step 3: [Description]

---

## Related Documents

**Relevant architecture documents:**
- [ ] [ARCHITECTURE.md](../../docs/ARCHITECTURE.md)
- [ ] [MEMORY_ARCHITECTURE.md](../../docs/MEMORY_ARCHITECTURE.md)
- [ ] [CAPABILITY_SYSTEM.md](../../docs/CAPABILITY_SYSTEM.md)
- [ ] [IPC_ARCHITECTURE.md](../../docs/IPC_ARCHITECTURE.md)
- [ ] [BOOT_ARCHITECTURE.md](../../docs/BOOT_ARCHITECTURE.md)
- [ ] Other: [Specify]

**Related issues/PRs:**
- Related to: #XXX
- Blocks: #XXX
- Blocked by: #XXX

**References:**
<!-- Links to research papers, related work, external resources -->

---

## Next Steps

- [ ] Discuss in this issue thread
- [ ] Create ADR document from [template](../../docs/adr/template.md)
- [ ] Submit ADR for review
- [ ] Update status based on decision

---

**Note:** Once this proposal is accepted, please create the ADR document using the template and link it from this issue.

