# Component Agent Instructions

<!-- BEGIN GENERATED:component (meta/bin/build, do not edit inside) -->
## Commercial B2B Agent (Tier 3 Specialist Worker) (`COMP-commercial-b2b-agent`)

Specialist agent executing domain state loops, commercial negotiations, and capability invocations.

Inspect any ID with `./meta/bin/ctx show <ID>`. Before editing, run `./meta/bin/ctx impact <files>`.

### Governing Invariants (must remain true)
- `INV-001` [P0, enforced by dependency_rule]: Cognitive Ingress Isolation & Zero Runtime LLM Bypass
- `INV-002` [P0, enforced by lint]: Zero Local Subprocess Execution in Workers & Pure SDK Kernel

### Forbidden Responsibilities & Shortcuts
- Local subprocess execution (INV-002 enforced via clippy.toml)
- Holding master database credentials or direct DB modifications
- Bypassing the 9-step commercial activation ceremony (FLOW-002)

### Canonical Implementation Patterns (Golden Exemplars)
- `PAT-002`: Zero-Trust ServiceRequestEnvelope Construction & Signing
- `PAT-005`: Multi-Turn Cognitive Planning Loop over /v1/responses

### Data & Wire Contracts
- `CTR-001`: TrustEnvelope & ServiceRequestEnvelope Wire Contract

### Hidden Couplings (breaks at a distance)
- `COUP-001`: Mesh Loopback Port Topology & Service Route Table
- `COUP-003`: Shared Ed25519 Keys & OIDC/JWKS Endpoints

### Past Agent Lessons
- `LES-001`: Direct Process Execution in Worker Agents

### Fast Verification Commands
- `cargo check --manifest-path commercial_b2b_agent/Cargo.toml --all-targets`
- `cargo clippy --manifest-path commercial_b2b_agent/Cargo.toml --all-targets -- -D warnings`
- `cargo test --manifest-path commercial_b2b_agent/Cargo.toml`
<!-- END GENERATED:component -->
