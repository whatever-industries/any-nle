# Working on Any NLE

- Start with [CONTRIBUTING.md](CONTRIBUTING.md) for setup and evidence rules,
  the [roadmap](docs/ROADMAP.md#where-work-is-tracked) for route status and
  holds, and the [source graph guide](docs/SOURCE-GRAPH-MIGRATION.md#boundaries)
  for placement of changes.
- Inspect Git status and preserve existing work. Keep changes and tests
  focused on the requested behavior. Keep the converter standalone.
- Use clean-room evidence and shareable synthetic fixtures. Preserve source
  inputs, exact clocks, media identity, and channel assignments. Keep
  unresolved facts explicit, report losses by occurrence, and reject
  malformed or contradictory inputs.
- Run the relevant public checks and report which evidence comes from
  structural tests, editor reopen, or rendered comparisons. With only a
  public clone, use the included tests and mark private or native checks
  pending when applicable.
- In the private maintainer checkout, also read the root workspace
  instructions and the **Local maintainer workflow** in
  `docs/PRIVATE-GITHUB.md`. That workflow covers ownership, knowledgebase
  pins, the full acceptance suite, and the guarded GitHub export.
