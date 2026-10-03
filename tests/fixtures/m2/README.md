# M2 structural vectors

Run `python3 scripts/m2-canonical-vectors.py` from any directory to reproduce
`canonical.json`. The generator uses Python SHA-384 and integer encoding, not
the Rust implementation under test. Public keys are selected from the existing
pinned NIST fixture; its source and full notice remain in `../pq/`.

These are canonical-byte/hash vectors, not valid payment or finality evidence.
All signature fields are intentionally zero and fail cryptographic verification.
See `docs/m2-canonical-profile.md` for the complete format and domain definitions.
