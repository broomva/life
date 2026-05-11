# Schema Mirror Provenance

**Source**: `github.com/warpdotdev/warp` `crates/warp_graphql_schema/api/schema.graphql`
**Pinned commit**: `5b047fc2fdb19b03f150973bbaa5497e87c18949`
**Last-touched commit (this file)**: `7f5a68932daa4408972bb6b9fe3bf0a62b15056c`
**Fetched**: 2026-05-11
**License**: AGPLv3 (per upstream LICENSE)

## Update procedure

When upstream Warp updates its schema:

1. Fetch the new file (`gh api ...`).
2. Run `cargo test -p lifegw --test warp_graphql_introspection` — failures show drift.
3. Update resolvers/tier1.rs and tier2.rs to match any T1/T2 op shape changes.
4. Update this file's pinned commit + fetch date.
5. Open a Linear ticket for the rebase under the M7-W umbrella.

The mirror is byte-identical to upstream. Any divergence is a bug in our copy, not a feature.
