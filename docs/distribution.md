# Dependency notices and source distribution

Original RinPQC source retains its MIT license in `LICENSE`. The M2 ML-DSA-87
implementation depends on `qp-rusty-crystals-dilithium =4.1.1`, licensed GPL-3.0
in its published manifest and archive. Its license is reproduced verbatim in
[`licenses/ML-DSA-GPL-3.0.txt`](../licenses/ML-DSA-GPL-3.0.txt). The package/version
exception in `deny.toml` implements the policy accepted in ADR 0002; other GPL
packages are not automatically allowed. The MIT package label does not describe
the distribution obligations of every linked component.

Distributors of a combined binary must comply with the applicable GPLv3 terms,
including notices, corresponding source and build material. Do not distribute a
binary with only the root MIT notice. Produce a source archive from the exact
build checkout with `bash scripts/package-source.sh /tmp/rinpqc-source.tar.gz`
and provide it with the binary, along with these notices and license text.
The archive includes node source, local patches, locked Rust dependency sources
and their notices, Cargo configuration for an offline build, and build instructions.
Keep any distribution-specific changes and build instructions with the source.

The Dockerfile places that archive, this notice, the MIT notice and GPL text under
`/usr/share/rinpqc/`. Preserve these files when redistributing an image or supply
the equivalent corresponding source alongside an extracted binary. OS packages
and the Rust/compiler toolchain have their own distribution terms. This document
records the project's distribution policy, not a legal audit.

The selected signature package pins `zeroize =1.8.2`, so the shared workspace
lockfile moves from 1.9.0 to 1.8.2. The lockfile checksum and normal advisory,
license and source checks remain mandatory. No algorithm fallback is enabled.
