// Default Bun version shipped with this Electrobun release.
// All platforms use the same version. Update this when bumping Bun.
//
// "canary" tracks Bun's Rust rewrite, which is published only via the rolling
// `canary` GitHub release tag (there is no `bun-v1.4.0-canary.N` release asset).
// The downloaders in build.ts and cli/index.ts special-case this value: a
// concrete version resolves to the `bun-v<version>` tag, "canary" resolves to
// the `canary` tag. The rolling tag is intentionally not pinned to a single
// SHA, so canary builds are not reproducible by version string alone.
export const BUN_VERSION = "canary";
