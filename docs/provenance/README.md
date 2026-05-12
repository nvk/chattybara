# provenance

Every compatibility-sensitive claim must carry a provenance label before it reaches public implementation code.

## Labels

- `clean-public`: public documentation, public trace, math derivation, or standard primitive.
- `clean-inferred`: inferred from controlled experiments and independently reproducible.
- `tainted-review`: known or suspected to come from disassembly, runtime memory inspection, leaked source, or license-restricted material.
- `blocked`: not usable in code or public docs without review by counsel.

## Merge Rule

Code may depend on `clean-public` and `clean-inferred` facts. It must not depend on `tainted-review` or `blocked` material. Tenuous details are quarantined, not disguised.
