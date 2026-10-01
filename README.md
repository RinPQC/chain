# RinPQC

A Rust payment-chain proof of concept built around **Malachite**, progressing from a working consensus network to post-quantum cryptography.

**Current stage: M1 durable payment state.** This README defines two implementation milestones and their completion criteria. It does not claim that either milestone is implemented, that the network is production-ready, or that its security has been independently audited.

This repository hosts the node implementation, integration tests, development-network configuration, and implementation documentation. Research, requirements, and architectural decision records live in [RinPQC/consensus](https://github.com/RinPQC/consensus).

## Build and run the scaffold

See [build instructions](docs/development/build.md) for prerequisites, `just setup` and the shared local/CI gate `just check`. The executable supports key generation, genesis/configuration validation and local initialization. Follow the [local identity walkthrough](docs/development/identities.md). The [payment executor](docs/development/payments.md) validates transfers and blocks in memory. The [application store](docs/development/storage.md) commits blocks and payment effects atomically and checks them on recovery. Network startup, consensus-driven settlement and payment RPC are not implemented yet.

## Objective

Build a small, understandable payment chain that:

- Executes valid payments consistently across independent nodes.
- Uses Malachite to agree on blocks and provide deterministic finality within its declared security assumptions.
- Persists state and recovers safely after interruptions.
- Separates payment logic, consensus integration, cryptography, networking, and storage.
- Can replace its initial classical cryptography with explicitly selected post-quantum constructions.

The first network is disposable development infrastructure. Its tokens have no promised monetary value or conversion into future mainnet allocations. The PoC does not establish permissionless admission, economic security, mobile-validator support, or production capacity.

## Milestones at a glance

| Milestone | Deliverable | Completion evidence |
| --- | --- | --- |
| **M1 — Working chain with basic consensus** | A persistent development network running simple payments with classical signatures. | Consistent finalized history and balances; invalid-payment rejection; restart, synchronization, and fault scenarios. |
| **M2 — Post-quantum cryptography integration** | The M1 functionality running with the selected PQ cryptography across required security paths. | Repeated correctness and fault tests; explicit cryptographic coverage; measured resource costs and revised operating parameters. |

M1 establishes the integration baseline. M2 re-evaluates that baseline after changing cryptographic assumptions, message sizes, and verification costs. M1 performance results are not PQ performance claims.

## Milestone 1 — Working chain with basic consensus

### Scope

**Node and consensus**

- A Rust node integrating a pinned Malachite release or commit through a dedicated integration layer.
- Four validators with separate keys and data directories, initially with equal voting power.
- A fixed validator set declared in genesis.
- Classical signatures for transactions and consensus messages.
- Block proposal, voting, and deterministic finality under the selected engine's assumptions.
- A configurable block-size limit and target block interval, initially exploring approximately five seconds. This is a normal-operation target, not a deadline guaranteed during failures.

**Payments and state**

- One native test token with initial balances defined in genesis.
- Transfers between accounts and a basic queue of pending transactions.
- Signature, balance, and replay-protection checks.
- Deterministic validation and execution of proposed payments against local state before supporting their inclusion in a finalized block.
- Persistent blocks, account state, and signing/recovery information needed for safe restart.

**Operation and access**

- Synchronization of a new or returning node with the development network.
- Minimal RPC and CLI support for key creation, payment submission, balance queries, transaction status, and chain height.
- Logs and basic metrics for consensus, synchronization, and failure diagnosis.
- Reproducible development-network configuration and operating instructions.

The initial setup should retain the history needed for its synchronization tests. Production pruning, archive incentives, and historical-data service guarantees are separate work.

### Completion criteria

| ID | Criterion |
| --- | --- |
| M1-01 | Correct nodes finalize the same history and derive identical account balances. |
| M1-02 | An accepted valid payment takes effect exactly once. Sender and recipient balances, transfer status, and replay-protection state commit atomically, including across a crash/restart: recovery must expose either the complete committed payment or none of its effects, never a partial transfer. Invalid signatures, insufficient funds, replay attempts, and conflicting double-spend attempts are rejected as required by the specified transaction rules. |
| M1-03 | With four equal-power validators and one unavailable, the remaining three continue finalizing blocks after applicable timeouts, under the declared network conditions. |
| M1-04 | A restarted node restores durable state, catches up, and safely resumes its duties without conflicting signing caused by lost or stale local state. |
| M1-05 | A new node synchronizes to the same verified development-chain state from the declared genesis and available history. |
| M1-06 | Finalization stops when the required quorum is unavailable. Partition and reconnection scenarios produce no conflicting finalized blocks in the tested traces. |
| M1-07 | Another developer can reproduce the network setup, payment flow, and principal failure scenarios from the repository documentation. |

Tests provide evidence about the implementation under specified conditions; they do not replace the consensus safety argument or prove correctness for every possible execution.

## Milestone 2 — Post-quantum cryptography integration

### Scope

**Signature and identity integration**

- Select a concrete ML-DSA parameter set and implementation after compatibility and license review.
- Integrate PQ signatures for payments, block proposals, and validator votes, including verification of finality evidence.
- Update key handling, account/address formats, transaction encoding, RPC, and CLI as required.
- Bind signatures to unambiguous contexts: network identity and message type, plus height, round, and voting phase where applicable.
- Review network identities and transport separately and replace classical dependencies where required by the agreed PQ security scope. Consensus-signature replacement alone does not complete this work.

**Cryptographic coverage and resource accounting**

- Inventory hashing, key generation, randomness, any proposer-selection mechanism, and remaining cryptographic dependencies.
- Record the required security property, chosen construction, implementation/version, and any unresolved limitation for each dependency.
- Adapt message, block, and verification-work limits to larger signatures and keys.
- Repeat M1 payment, fault, restart, and synchronization scenarios with PQ enabled.
- Measure throughput, finality latency, CPU, peak memory, bandwidth, and persistent-data growth on specified hardware and network configurations.
- Select PoC operating parameters from those measurements rather than inheriting classical-signature results.

### Completion criteria

| ID | Criterion |
| --- | --- |
| M2-01 | The network executes payments and finalizes blocks using the selected PQ signatures for all required transaction and consensus authorization paths. |
| M2-02 | Classical signatures are rejected wherever the PQ rules require their replacement; no undocumented classical authorization bypass remains. |
| M2-03 | Mandatory identity, transport, and other cryptographic obligations have an explicit disposition and verified integration where required. An unresolved mandatory dependency blocks milestone completion. |
| M2-04 | The M1 correctness and recovery scenarios pass under the PQ configuration, including malformed or invalid PQ signature cases. |
| M2-05 | Resource measurements identify the tested workload, cryptographic artifacts, validator count, hardware, network conditions, and observation period. Block and timing parameters are adjusted accordingly. |
| M2-06 | Documentation states the demonstrated scope, security assumptions, and remaining limitations without presenting the PoC as an audited or universally quantum-secure system. |

### Network reset boundary

The planned transition to M2 uses **a new genesis and fresh test state**. Preserving the M1 chain's balances, accounts, keys, or history across a cryptographic migration is outside this PoC.

A later migration of a persistent network would require a separate specification covering authorization, historical verification, key replacement, and activation rules.

## Integration and dependency discipline

Malachite is an external dependency, not the definition of the complete chain. Our application remains responsible for financial validity, account authorization, state persistence, and the rules that determine the validator set.

- Pin the engine version or commit and commit the dependency lockfile.
- Keep the integration layer small and avoid modifying the consensus engine where public interfaces suffice.
- Version wire formats and signing contexts; avoid hard-coding classical key and signature sizes into the application model.
- Review upstream updates through dedicated PRs, including changes to consensus behavior, encoding, storage compatibility, and recovery.
- Do not equate updating a dependency with safely upgrading a running network.

The selected version's maturity, API, license, and relevant security limitations must be recorded before implementation depends on it. This scope does not select a specific Malachite revision or ML-DSA parameter set.

## Outside these milestones

| Area | Excluded work |
| --- | --- |
| Permissionless participation | Dynamic admission, validator-set changes, staking, delegation, rewards, and slashing. |
| Token economics | Production supply policy, contributor allocations, and mainnet economic incentives. |
| Advanced proposer policies | Committees, randomized proposer selection, and additional proposer cooldown rules. |
| Additional financial functionality | Multisig accounts, external assets, bridges, NEAR integration, and external liquidity. |
| Execution expansion | ZK proofs, smart contracts, and EVM support. ZK is not a planned dependency of this PoC. |
| Production operations | Production governance and upgrade mechanisms, mobile-validation guarantees, and an archive-service/incentive model. |
| Historical migration | Preserving a live classical-signature network through the transition to PQ. |

**The confirmed PoC scope is one native test coin and payments only.** Atomic asset exchange is excluded from these milestones. Earlier research included exchange in the initial financial scope; this narrower PoC does not permanently reject exchange functionality for the project.

## Integration baseline

[ADR 0001 — Malachite baseline for M1](docs/decisions/0001-malachite-baseline.md) records the pinned engine revision, application interface, recovery obligations, and PQ integration boundaries. Status and evidence limits are recorded in the ADR.

## Implementation workflow

Each milestone should be broken into bounded issues linked to its completion criteria. Implementation PRs should explain the behavior changed, evidence collected, and known limitations.

Architectural decisions remain explicit records in the research repository. Completing a PoC task does not silently approve production economics, security assumptions, or features outside this scope.

The integration baseline is recorded in ADR 0001. [M1 payment, genesis and block semantics](docs/specs/m1-payment-semantics.md) specifies the proposed application contract and deterministic vectors for issue #4. These documents introduce no running implementation.
